#[macro_export]
macro_rules! tokio_spawn {
    ($name:expr, $future:expr) => {
        tokio::task::Builder::new().name($name).spawn($future).unwrap()
    };
}

#[macro_export]
macro_rules! tokio_spawn_blocking {
    ($name:expr, $future:expr) => {
        tokio::task::Builder::new().name($name).spawn_blocking($future).unwrap()
    };
}
