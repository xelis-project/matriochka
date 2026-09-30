/// A caller-defined input error stored by type in the runtime context.
///
/// `I` may be a `thiserror` enum, or any owned `Send + Sync` value.
pub struct InputError<I: 'static>(pub I);

runtime_context::tid! { impl<'a, I: 'static> TidAble<'a> for InputError<I> }
