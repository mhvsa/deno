// Copyright 2018-2019 the Deno authors. All rights reserved. MIT license.
use super::dispatch_json::{Deserialize, JsonOp, Value};
use crate::fs as deno_fs;
use crate::ops::json_op;
use crate::resources;
use crate::state::ThreadSafeState;
use deno::*;
use futures::Future;
use std;
use std::convert::From;
use tokio;

pub fn init(i: &mut Isolate, s: &ThreadSafeState) {
  i.register_op("open", s.core_op(json_op(s.stateful_op(op_open))));
  i.register_op("close", s.core_op(json_op(s.stateful_op(op_close))));
  i.register_op("seek", s.core_op(json_op(s.stateful_op(op_seek))));
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenArgs {
  promise_id: Option<u64>,
  filename: String,
  capability: OpenCapability,
}
#[derive(Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
struct OpenCapability {
  read: bool,
  write: bool,
  create: bool,
  truncate: bool,
  append: bool,
  create_new: bool,
}

fn op_open(
  state: &ThreadSafeState,
  args: Value,
  _zero_copy: Option<PinnedBuf>,
) -> Result<JsonOp, ErrBox> {
  let args: OpenArgs = serde_json::from_value(args)?;
  let (filename, filename_) = deno_fs::resolve_from_cwd(&args.filename)?;
  let capability = args.capability;

  let mut open_options = tokio::fs::OpenOptions::new();

  open_options
    .read(capability.read)
    .create(capability.create)
    .write(capability.write)
    .truncate(capability.truncate)
    .append(capability.append)
    .create_new(capability.create_new);

  if capability.read {
    state.check_read(&filename_)?;
  }

  if capability.write || capability.append {
    state.check_write(&filename_)?;
  }

  let is_sync = args.promise_id.is_none();
  let op = open_options.open(filename).map_err(ErrBox::from).and_then(
    move |fs_file| {
      let resource = resources::add_fs_file(fs_file);
      futures::future::ok(json!(resource.rid))
    },
  );

  if is_sync {
    let buf = op.wait()?;
    Ok(JsonOp::Sync(buf))
  } else {
    Ok(JsonOp::Async(Box::new(op)))
  }
}

#[derive(Deserialize)]
struct CloseArgs {
  rid: i32,
}

fn op_close(
  _state: &ThreadSafeState,
  args: Value,
  _zero_copy: Option<PinnedBuf>,
) -> Result<JsonOp, ErrBox> {
  let args: CloseArgs = serde_json::from_value(args)?;

  let resource = resources::lookup(args.rid as u32)?;
  resource.close();
  Ok(JsonOp::Sync(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeekArgs {
  promise_id: Option<u64>,
  rid: i32,
  offset: i32,
  whence: i32,
}

fn op_seek(
  _state: &ThreadSafeState,
  args: Value,
  _zero_copy: Option<PinnedBuf>,
) -> Result<JsonOp, ErrBox> {
  let args: SeekArgs = serde_json::from_value(args)?;

  let resource = resources::lookup(args.rid as u32)?;
  let op = resources::seek(resource, args.offset, args.whence as u32)
    .and_then(move |_| futures::future::ok(json!({})));
  if args.promise_id.is_none() {
    let buf = op.wait()?;
    Ok(JsonOp::Sync(buf))
  } else {
    Ok(JsonOp::Async(Box::new(op)))
  }
}
