use super::dispatch_json::{Deserialize, JsonOp, Value};
use crate::ops::json_op;
use crate::state::ThreadSafeState;
use deno::{PinnedBuf, ErrBox, Isolate};
use tokio::timer::Delay;
use tokio::prelude::*;
use rand::random;
use std::time::{Duration, Instant};


pub fn init(i: &mut Isolate, s: &ThreadSafeState) {
  i.register_op("add", s.core_op(json_op(s.stateful_op(op_add))));
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddArgs {
  promise_id: Option<u64>,
  a: f64,
  b: f64,
}

struct AddFuture {
  a: f64,
  b: f64,
  delay: Delay,
}

impl Future for AddFuture {
  type Item = Value;
  type Error = ErrBox;

  fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
    try_ready!(self.delay.poll().map_err(ErrBox::from));
    Ok(Async::Ready(json!(self.a + self.b)))
  }
}

fn op_add(
  _state: &ThreadSafeState,
  args: Value,
  _zero_copy: Option<PinnedBuf>,
) -> Result<JsonOp, ErrBox> {
  let args: AddArgs = serde_json::from_value(args)?;
  let is_sync = args.promise_id.is_none();
  if is_sync {
    Ok(JsonOp::Sync(json!(args.a + args.b)))
  } else {
    let mills : u64 = (random::<f64>() * 5000.0 + 1000.0).floor() as u64;
    let when = Instant::now() + Duration::from_millis(mills);
    let delay = Delay::new(when);
    let op = AddFuture {
      a: args.a,
      b: args.b,
      delay
    };
    Ok(JsonOp::Async(Box::new(op)))
  }
}
