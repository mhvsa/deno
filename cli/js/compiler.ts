// Copyright 2018-2020 the Deno authors. All rights reserved. MIT license.
// TODO(ry) Combine this implementation with //deno_typescript/compiler_main.js

// these are imported for their side effects
import "./globals.ts";
import "./ts_global.d.ts";

import { TranspileOnlyResult } from "./compiler_api.ts";
import { oldProgram } from "./compiler_bootstrap.ts";
import { setRootExports } from "./compiler_bundler.ts";
import {
  defaultBundlerOptions,
  defaultRuntimeCompileOptions,
  defaultTranspileOptions,
  Host
} from "./compiler_host.ts";
import {
  processImports,
  processLocalImports,
  resolveModules
} from "./compiler_imports.ts";
import {
  createWriteFile,
  CompilerRequestType,
  convertCompilerOptions,
  ignoredDiagnostics,
  WriteFileState,
  processConfigureResponse
} from "./compiler_util.ts";
import { Diagnostic } from "./diagnostics.ts";
import { fromTypeScriptDiagnostic } from "./diagnostics_util.ts";
import * as os from "./os.ts";
import { assert } from "./util.ts";
import * as util from "./util.ts";
import { window as self } from "./window.ts";
import { postMessage, workerClose, workerMain } from "./workers.ts";

interface CompilerRequestCompile {
  type: CompilerRequestType.Compile;
  rootNames: string[];
  // TODO(ry) add compiler config to this interface.
  // options: ts.CompilerOptions;
  configPath?: string;
  config?: string;
  bundle?: boolean;
  outFile?: string;
}

interface CompilerRequestRuntimeCompile {
  type: CompilerRequestType.RuntimeCompile;
  rootName: string;
  sources?: Record<string, string>;
  bundle?: boolean;
  options?: string;
}

interface CompilerRequestRuntimeTranspile {
  type: CompilerRequestType.RuntimeTranspile;
  sources: Record<string, string>;
  options?: string;
}

/** The format of the work message payload coming from the privileged side */
type CompilerRequest =
  | CompilerRequestCompile
  | CompilerRequestRuntimeCompile
  | CompilerRequestRuntimeTranspile;

/** The format of the result sent back when doing a compilation. */
interface CompileResult {
  emitSkipped: boolean;
  diagnostics?: Diagnostic;
}

// bootstrap the runtime environment, this gets called as the isolate is setup
self.denoMain = function denoMain(compilerType?: string): void {
  os.start(true, compilerType ?? "TS");
};

// bootstrap the worker environment, this gets called as the isolate is setup
self.workerMain = workerMain;

// provide the "main" function that will be called by the privileged side when
// lazy instantiating the compiler web worker
self.compilerMain = function compilerMain(): void {
  // workerMain should have already been called since a compiler is a worker.
  self.onmessage = async ({
    data: request
  }: {
    data: CompilerRequest;
  }): Promise<void> => {
    switch (request.type) {
      // `Compile` are requests from the internals to Deno, generated by both
      // the `run` and `bundle` sub command.
      case CompilerRequestType.Compile: {
        const { bundle, config, configPath, outFile, rootNames } = request;
        util.log(">>> compile start", {
          rootNames,
          type: CompilerRequestType[request.type]
        });

        // This will recursively analyse all the code for other imports,
        // requesting those from the privileged side, populating the in memory
        // cache which will be used by the host, before resolving.
        const resolvedRootModules = await processImports(
          rootNames.map(rootName => [rootName, rootName])
        );

        // When a programme is emitted, TypeScript will call `writeFile` with
        // each file that needs to be emitted.  The Deno compiler host delegates
        // this, to make it easier to perform the right actions, which vary
        // based a lot on the request.  For a `Compile` request, we need to
        // cache all the files in the privileged side if we aren't bundling,
        // and if we are bundling we need to enrich the bundle and either write
        // out the bundle or log it to the console.
        const state: WriteFileState = {
          type: request.type,
          bundle,
          host: undefined,
          outFile,
          rootNames
        };
        const writeFile = createWriteFile(state);

        const host = (state.host = new Host({ bundle, writeFile }));
        let diagnostics: readonly ts.Diagnostic[] | undefined;

        // if there is a configuration supplied, we need to parse that
        if (config && config.length && configPath) {
          const configResult = host.configure(configPath, config);
          diagnostics = processConfigureResponse(configResult, configPath);
        }

        let emitSkipped = true;
        // if there was a configuration and no diagnostics with it, we will continue
        // to generate the program and possibly emit it.
        if (!diagnostics || (diagnostics && diagnostics.length === 0)) {
          const options = host.getCompilationSettings();
          const program = ts.createProgram({
            rootNames,
            options,
            host,
            oldProgram
          });

          diagnostics = ts
            .getPreEmitDiagnostics(program)
            .filter(({ code }) => !ignoredDiagnostics.includes(code));

          // We will only proceed with the emit if there are no diagnostics.
          if (diagnostics && diagnostics.length === 0) {
            if (bundle) {
              // we only support a single root module when bundling
              assert(resolvedRootModules.length === 1);
              // warning so it goes to stderr instead of stdout
              console.warn(`Bundling "${resolvedRootModules[0]}"`);
              setRootExports(program, resolvedRootModules[0]);
            }
            const emitResult = program.emit();
            emitSkipped = emitResult.emitSkipped;
            // emitResult.diagnostics is `readonly` in TS3.5+ and can't be assigned
            // without casting.
            diagnostics = emitResult.diagnostics;
          }
        }

        const result: CompileResult = {
          emitSkipped,
          diagnostics: diagnostics.length
            ? fromTypeScriptDiagnostic(diagnostics)
            : undefined
        };
        postMessage(result);

        util.log("<<< compile end", {
          rootNames,
          type: CompilerRequestType[request.type]
        });
        break;
      }
      case CompilerRequestType.RuntimeCompile: {
        // `RuntimeCompile` are requests from a runtime user, both compiles and
        // bundles.  The process is similar to a request from the privileged
        // side, but also returns the output to the on message.
        const { rootName, sources, options, bundle } = request;

        util.log(">>> runtime compile start", {
          rootName,
          bundle,
          sources: sources ? Object.keys(sources) : undefined
        });

        const resolvedRootName = sources
          ? rootName
          : resolveModules([rootName])[0];

        const rootNames = sources
          ? processLocalImports(sources, [[resolvedRootName, resolvedRootName]])
          : await processImports([[resolvedRootName, resolvedRootName]]);

        const state: WriteFileState = {
          type: request.type,
          bundle,
          host: undefined,
          rootNames,
          sources,
          emitMap: {},
          emitBundle: undefined
        };
        const writeFile = createWriteFile(state);

        const host = (state.host = new Host({ bundle, writeFile }));
        const compilerOptions = [defaultRuntimeCompileOptions];
        if (options) {
          compilerOptions.push(convertCompilerOptions(options));
        }
        if (bundle) {
          compilerOptions.push(defaultBundlerOptions);
        }
        host.mergeOptions(...compilerOptions);

        const program = ts.createProgram({
          rootNames,
          options: host.getCompilationSettings(),
          host,
          oldProgram
        });

        if (bundle) {
          setRootExports(program, rootNames[0]);
        }

        const diagnostics = ts
          .getPreEmitDiagnostics(program)
          .filter(({ code }) => !ignoredDiagnostics.includes(code));

        const emitResult = program.emit();

        assert(
          emitResult.emitSkipped === false,
          "Unexpected skip of the emit."
        );
        const { items } = fromTypeScriptDiagnostic(diagnostics);
        const result = [
          items && items.length ? items : undefined,
          bundle ? state.emitBundle : state.emitMap
        ];
        postMessage(result);

        assert(state.emitMap);
        util.log("<<< runtime compile finish", {
          rootName,
          sources: sources ? Object.keys(sources) : undefined,
          bundle,
          emitMap: Object.keys(state.emitMap)
        });

        break;
      }
      case CompilerRequestType.RuntimeTranspile: {
        const result: Record<string, TranspileOnlyResult> = {};
        const { sources, options } = request;
        const compilerOptions = options
          ? Object.assign(
              {},
              defaultTranspileOptions,
              convertCompilerOptions(options)
            )
          : defaultTranspileOptions;

        for (const [fileName, inputText] of Object.entries(sources)) {
          const { outputText: source, sourceMapText: map } = ts.transpileModule(
            inputText,
            {
              fileName,
              compilerOptions
            }
          );
          result[fileName] = { source, map };
        }
        postMessage(result);

        break;
      }
      default:
        util.log(
          `!!! unhandled CompilerRequestType: ${
            (request as CompilerRequest).type
          } (${CompilerRequestType[(request as CompilerRequest).type]})`
        );
    }

    // The compiler isolate exits after a single message.
    workerClose();
  };
};

self.wasmCompilerMain = function wasmCompilerMain(): void {
  // workerMain should have already been called since a compiler is a worker.
  self.onmessage = async ({
    data: binary
  }: {
    data: string;
  }): Promise<void> => {
    const buffer = util.base64ToUint8Array(binary);
    // @ts-ignore
    const compiled = await WebAssembly.compile(buffer);

    util.log(">>> WASM compile start");

    const importList = Array.from(
      // @ts-ignore
      new Set(WebAssembly.Module.imports(compiled).map(({ module }) => module))
    );
    const exportList = Array.from(
      // @ts-ignore
      new Set(WebAssembly.Module.exports(compiled).map(({ name }) => name))
    );

    postMessage({ importList, exportList });

    util.log("<<< WASM compile end");

    // The compiler isolate exits after a single message.
    workerClose();
  };
};
