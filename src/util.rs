use serde_yaml::Value;

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

pub fn merge_yaml(a: &mut Value, b: &Value) {
    match (a, b) {
        (Value::Mapping(map_a), Value::Mapping(map_b)) => {
            for (k, v_b) in map_b {
                match map_a.get_mut(k) {
                    Some(v_a) => merge_yaml(v_a, v_b),
                    None => {
                        map_a.insert(k.clone(), v_b.clone());
                    }
                }
            }
        }
        (Value::Sequence(seq_a), Value::Sequence(seq_b)) => {
            seq_a.extend(seq_b.clone());
        }
        (a, b) => {
            *a = b.clone();
        }
    }
}
