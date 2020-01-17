// Copyright 2018-2020 the Deno authors. All rights reserved. MIT license.
use super::dispatch_json::{Deserialize, JsonOp, Value};
use super::io::StreamResource;
use crate::deno_error::bad_resource;
use crate::deno_error::DenoError;
use crate::deno_error::ErrorKind;
use crate::fs as deno_fs;
use crate::ops::json_op;
use crate::state::ThreadSafeState;
use deno_core::*;
use futures::future::FutureExt;
use std;
use std::convert::From;
use std::io::SeekFrom;
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
  capability: OpenOptions,
}
#[derive(Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
struct OpenOptions {
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
  let state_ = state.clone();
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

  let fut = async move {
    let fs_file = open_options.open(filename).await?;
    let mut table = state_.lock_resource_table();
    let rid = table.add("fsFile", Box::new(StreamResource::FsFile(fs_file)));
    Ok(json!(rid))
  };

  if is_sync {
    let buf = futures::executor::block_on(fut)?;
    Ok(JsonOp::Sync(buf))
  } else {
    Ok(JsonOp::Async(fut.boxed()))
  }
}

#[derive(Deserialize)]
struct CloseArgs {
  rid: i32,
}

fn op_close(
  state: &ThreadSafeState,
  args: Value,
  _zero_copy: Option<PinnedBuf>,
) -> Result<JsonOp, ErrBox> {
  let args: CloseArgs = serde_json::from_value(args)?;

  let mut table = state.lock_resource_table();
  table.close(args.rid as u32).ok_or_else(bad_resource)?;
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
  state: &ThreadSafeState,
  args: Value,
  _zero_copy: Option<PinnedBuf>,
) -> Result<JsonOp, ErrBox> {
  let args: SeekArgs = serde_json::from_value(args)?;
  let rid = args.rid as u32;
  let offset = args.offset;
  let whence = args.whence as u32;
  // Translate seek mode to Rust repr.
  let seek_from = match whence {
    0 => SeekFrom::Start(offset as u64),
    1 => SeekFrom::Current(i64::from(offset)),
    2 => SeekFrom::End(i64::from(offset)),
    _ => {
      return Err(ErrBox::from(DenoError::new(
        ErrorKind::InvalidSeekMode,
        format!("Invalid seek mode: {}", whence),
      )));
    }
  };

  let mut table = state.lock_resource_table();
  let resource = table
    .get_mut::<StreamResource>(rid)
    .ok_or_else(bad_resource)?;

  let tokio_file = match resource {
    StreamResource::FsFile(ref mut file) => file,
    _ => return Err(bad_resource()),
  };
  let mut file = futures::executor::block_on(tokio_file.try_clone())?;

  let fut = async move {
    file.seek(seek_from).await?;
    Ok(json!({}))
  };

  if args.promise_id.is_none() {
    let buf = futures::executor::block_on(fut)?;
    Ok(JsonOp::Sync(buf))
  } else {
    Ok(JsonOp::Async(fut.boxed()))
  }
}
