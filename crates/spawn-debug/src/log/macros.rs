//! Logging macros. Each is no-return, infallible, and callable from any thread.
//!
//! Expansion contract: a `const` compile-time floor check comes first; when a
//! level is above the floor the message branch is dead code the optimizer
//! removes and the format arguments are never evaluated. Otherwise the runtime
//! `Logger::enabled` check (one atomic load + branch) gates record construction.

/// Internal: emit one record after both the compile-time and runtime gates pass.
/// Not part of the public contract; used by the level macros below.
#[macro_export]
#[doc(hidden)]
macro_rules! __spawn_log {
    ($level:expr, $target:expr, $($arg:tt)+) => {{
        // Compile-time floor: when stripped, `$($arg)+` is never evaluated
        // because the entire block is unreachable and removed.
        if !$crate::log::COMPILE_OFF
            && ($level as u8) <= ($crate::log::COMPILE_MAX_LEVEL as u8)
            && $crate::log::Logger::enabled($level, $target)
        {
            let record = $crate::log::LogRecord {
                level: $level,
                target: $target,
                message: ::core::format_args!($($arg)+),
                timestamp: $crate::log::Logger::elapsed(),
                thread: $crate::log::thread_tag(),
            };
            $crate::log::Logger::dispatch(&record);
        }
    }};
}

/// Internal: route an optional `target:` prefix to `__spawn_log`.
#[macro_export]
#[doc(hidden)]
macro_rules! __spawn_log_dispatch {
    ($level:expr, target: $target:expr, $($arg:tt)+) => {
        $crate::__spawn_log!($level, $target, $($arg)+)
    };
    ($level:expr, $($arg:tt)+) => {
        $crate::__spawn_log!($level, ::core::module_path!(), $($arg)+)
    };
}

/// Log at `Error`. Optional leading `target: "..."`, else `module_path!()`.
#[macro_export]
macro_rules! spawn_error {
    ($($arg:tt)+) => {
        $crate::__spawn_log_dispatch!($crate::log::LogLevel::Error, $($arg)+)
    };
}

/// Log at `Warn`. Optional leading `target: "..."`, else `module_path!()`.
#[macro_export]
macro_rules! spawn_warn {
    ($($arg:tt)+) => {
        $crate::__spawn_log_dispatch!($crate::log::LogLevel::Warn, $($arg)+)
    };
}

/// Log at `Info`. Optional leading `target: "..."`, else `module_path!()`.
#[macro_export]
macro_rules! spawn_info {
    ($($arg:tt)+) => {
        $crate::__spawn_log_dispatch!($crate::log::LogLevel::Info, $($arg)+)
    };
}

/// Log at `Debug`. Optional leading `target: "..."`, else `module_path!()`.
#[macro_export]
macro_rules! spawn_debug {
    ($($arg:tt)+) => {
        $crate::__spawn_log_dispatch!($crate::log::LogLevel::Debug, $($arg)+)
    };
}

/// Log at `Trace`. Optional leading `target: "..."`, else `module_path!()`.
#[macro_export]
macro_rules! spawn_trace {
    ($($arg:tt)+) => {
        $crate::__spawn_log_dispatch!($crate::log::LogLevel::Trace, $($arg)+)
    };
}
