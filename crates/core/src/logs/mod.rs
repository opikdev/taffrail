mod store;
mod tail;

pub use store::LogLine;
pub(crate) use tail::start;
pub use tail::{LogProgress, LogReader, LogTail, LogTarget, MAX_TAIL_PODS};
