mod store;
mod tail;

pub use store::LogLine;
pub use tail::{Following, LogProgress, LogReader, LogTail, LogTarget, MAX_TAIL_PODS};
pub(crate) use tail::{follow, start};
