import { sendSync } from "./dispatch_json.ts";
import * as dispatch from "./dispatch.ts";

export function addSync(a: number, b: number) : number {
  return sendSync(dispatch.OP_ADD, {a, b})
}
