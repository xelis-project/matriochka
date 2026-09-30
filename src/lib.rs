//! Errors wrapped with caller-defined context at each application boundary.
//!
//! [`Error`] preserves the original error and its sources. Add context with
//! [`ResultExt::context`] or [`ResultExt::with_context`], then use `{error:#}`
//! to display the entire chain, from the outermost context to the root cause.
//! Context is an annotated error chain, not a captured runtime backtrace.
//!
//! ```
//! use matriochka::{Error, ResultExt};
//!
//! fn load_account(id: u64) -> Result<String, Error> {
//!     std::fs::read_to_string(format!("accounts/{id}"))
//!         .with_context(|| format!("loading account {id}"))
//! }
//!
//! fn rpc_account(id: u64) -> Result<String, Error> {
//!     load_account(id).context("handling account RPC")
//! }
//! ```

mod chain;
mod context;
mod error;
mod input_error;
mod result_ext;

pub use chain::Chain;
pub use error::Error;
pub use input_error::InputError;
pub use result_ext::ResultExt;
pub use runtime_context;
