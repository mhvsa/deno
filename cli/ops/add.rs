use super::dispatch_json::{blocking_json, Deserialize, JsonOp, Value};
use crate::ops::json_op;
use crate::state::ThreadSafeState;
use deno::{PinnedBuf, ErrBox, Isolate};

pub fn init(i: &mut Isolate, s: &ThreadSafeState) {
  i.register_op("add", s.core_op(json_op(s.stateful_op(op_add))));
}

#[derive(Deserialize)]
struct AddArgs {
  a: f64,
  b: f64,
}

fn op_add(
  _state: &ThreadSafeState,
  args: Value,
  _zero_copy: Option<PinnedBuf>,
) -> Result<JsonOp, ErrBox> {
  let args: AddArgs = serde_json::from_value(args)?;
  Ok(JsonOp::Sync(json!(args.a + args.b)))
}
