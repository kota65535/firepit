
#[macro_export]
macro_rules! tokio_spawn {
    ($name:literal, {$($field:ident = $value:expr),*}, $future:expr) => {{
        use tracing::Instrument;
        tokio::task::Builder::new()
            .name(&format!(
                "{} {}",
                $name,
                vec![$(format!("{}={:?}", stringify!($field), $value)),*].join(", ")
            ))
            .spawn($future.instrument(tracing::info_span!($name, $($field = $value),*)))
            .unwrap()
    }};

    ($name:literal, $future:expr) => {{
        use tracing::Instrument;
        tokio::task::Builder::new()
            .name($name)
            .spawn($future.instrument(tracing::info_span!($name)))
            .unwrap()
    }};
}

#[macro_export]
macro_rules! tokio_spawn_blocking {
   ($name:literal, {$($field:ident = $value:expr),*}, $block:expr) => {{
       tokio::task::Builder::new()
            .name(&format!(
                "{} {}",
                $name,
                vec![$(format!("{} = {:?}", stringify!($field), $value)),*].join(", ")
            ))
           .spawn_blocking(move || {
                let span = tracing::info_span!($name, $($field = $value),*);
                let _guard = span.enter();
                ($block)()
            })
            .unwrap()
   }};
   ($name:literal, $block:expr) => {{
       tokio::task::Builder::new()
           .name($name)
           .spawn_blocking(move || {
                let span = tracing::info_span!($name);
                let _guard = span.enter();
                ($block)()
            })
            .unwrap()
   }};
}
