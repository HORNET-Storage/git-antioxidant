/// The span of builds without `tracing`, which records nothing. Like the span of builds with `tracing`, it ends when
/// it is dropped, so that ending a span early with `drop()` means the same in both builds.
#[doc(hidden)]
#[derive(Clone)]
pub struct Span;

impl Drop for Span {
    fn drop(&mut self) {}
}

impl Span {
    /// A no-op
    pub fn record<V>(&self, _field: &str, _value: V) -> &Self {
        self
    }
}

/// A macro to create a span.
#[macro_export]
macro_rules! span {
    (target: $target:expr, $lvl:expr, $name:expr, $($fields:tt)*) => {
        $crate::Span
    };
    (target: $target:expr, $lvl:expr, $name:expr) => {
        $crate::span!(target: $target, $lvl, $name,)
    };
    ($lvl:expr, $name:expr, $($fields:tt)*) => {
        $crate::span!(
            target: module_path!(),
            $lvl,
            $name,
            $($fields)*
        )
    };
    ($lvl:expr, $name:expr) => {
        $crate::span!(
            target: module_path!(),
            $lvl,
            $name,
        )
    };
}

/// Create an event with the given level.
#[macro_export]
macro_rules! event {
    (target: $target:expr, $lvl:expr, { $($fields:tt)* } )=> (
        {}
    );
    (target: $target:expr, $lvl:expr, { $($fields:tt)* }, $($arg:tt)+ ) => (
        $crate::event!(
            target: $target,
            $lvl,
            { message = format_args!($($arg)+), $($fields)* }
        )
    );
    (target: $target:expr, $lvl:expr, $($k:ident).+ = $($fields:tt)* ) => (
        $crate::event!(target: $target, $lvl, { $($k).+ = $($fields)* })
    );
    (target: $target:expr, $lvl:expr, $($arg:tt)+ ) => (
        $crate::event!(target: $target, $lvl, { $($arg)+ })
    );
    ( $lvl:expr, { $($fields:tt)* }, $($arg:tt)+ ) => (
        $crate::event!(
            target: module_path!(),
            $lvl,
            { message = format_args!($($arg)+), $($fields)* }
        )
    );
    ($lvl:expr, $($k:ident).+ = $($field:tt)*) => (
        $crate::event!(
            target: module_path!(),
            $lvl,
            { $($k).+ = $($field)*}
        )
    );
    ($lvl:expr, $($k:ident).+, $($field:tt)*) => (
        $crate::event!(
            target: module_path!(),
            $lvl,
            { $($k).+, $($field)*}
        )
    );
    ($lvl:expr, ?$($k:ident).+, $($field:tt)*) => (
        $crate::event!(
            target: module_path!(),
            $lvl,
            { ?$($k).+, $($field)*}
        )
    );
    ($lvl:expr, %$($k:ident).+, $($field:tt)*) => (
        $crate::event!(
            target: module_path!(),
            $lvl,
            { %$($k).+, $($field)*}
        )
    );
    ($lvl:expr, ?$($k:ident).+) => (
        $crate::event!($lvl, ?$($k).+,)
    );
    ($lvl:expr, %$($k:ident).+) => (
        $crate::event!($lvl, %$($k).+,)
    );
    ($lvl:expr, $($k:ident).+) => (
        $crate::event!($lvl, $($k).+,)
    );
    ( $lvl:expr, $($arg:tt)+ ) => (
        $crate::event!(target: module_path!(), $lvl, { $($arg)+ })
    );
}
