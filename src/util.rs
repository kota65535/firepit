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

pub fn merge_yaml(a: &mut Value, b: &Value, overwrite: bool) {
    if b.is_null() {
        return;
    }

    match (a, b) {
        (Value::Mapping(map_a), Value::Mapping(map_b)) => {
            for (k, v_b) in map_b {
                match map_a.get_mut(k) {
                    Some(v_a) => merge_yaml(v_a, v_b, overwrite),
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
            if a.is_null() || overwrite {
                *a = b.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use serde_yaml::Value;

    #[rstest]
    #[case(true)]
    #[case(false)]
    fn test_merge_yaml_mappings(#[case] overwrite: bool) {
        let mut a: Value = serde_yaml::from_str(
            r#"
            key1: value1
            key2: 
              nested_key1: nested_value1
        "#,
        )
        .unwrap();

        let b: Value = serde_yaml::from_str(
            r#"
            key2: 
              nested_key2: nested_value2
            key3: value3
        "#,
        )
        .unwrap();

        merge_yaml(&mut a, &b, overwrite);

        let expected: Value = serde_yaml::from_str(
            r#"
            key1: value1
            key2:
              nested_key1: nested_value1
              nested_key2: nested_value2
            key3: value3
        "#,
        )
        .unwrap();

        assert_eq!(a, expected);
    }

    #[rstest]
    #[case(true)]
    #[case(false)]
    fn test_merge_yaml_sequences(#[case] overwrite: bool) {
        let mut a: Value = serde_yaml::from_str(
            r#"
            - value1
            - value2
        "#,
        )
        .unwrap();

        let b: Value = serde_yaml::from_str(
            r#"
            - value3
            - value4
        "#,
        )
        .unwrap();

        merge_yaml(&mut a, &b, overwrite);

        let expected: Value = serde_yaml::from_str(
            r#"
            - value1
            - value2
            - value3
            - value4
        "#,
        )
        .unwrap();

        assert_eq!(a, expected);
    }

    #[rstest]
    #[case(true)]
    #[case(false)]
    fn test_merge_yaml_overwrite(#[case] overwrite: bool) {
        let mut a: Value = serde_yaml::from_str(
            r#"
            key: old_value
        "#,
        )
        .unwrap();

        let b: Value = serde_yaml::from_str(
            r#"
            key: new_value
        "#,
        )
        .unwrap();

        merge_yaml(&mut a, &b, overwrite);

        let expected: Value = if overwrite {
            serde_yaml::from_str(
                r#"
            key: new_value
        "#,
            )
            .unwrap()
        } else {
            serde_yaml::from_str(
                r#"
            key: old_value
        "#,
            )
            .unwrap()
        };

        assert_eq!(a, expected);
    }

    #[rstest]
    #[case(true)]
    #[case(false)]
    fn test_merge_yaml_empty_a(#[case] overwrite: bool) {
        let mut a: Value = serde_yaml::from_str(r#""#).unwrap();

        let b: Value = serde_yaml::from_str(
            r#"
            key: value
        "#,
        )
        .unwrap();

        merge_yaml(&mut a, &b, overwrite);

        let expected: Value = serde_yaml::from_str(
            r#"
            key: value
        "#,
        )
        .unwrap();

        assert_eq!(a, expected);
    }

    #[rstest]
    #[case(true)]
    #[case(false)]
    fn test_merge_yaml_empty_b(#[case] overwrite: bool) {
        let mut a: Value = serde_yaml::from_str(
            r#"
            key: value
        "#,
        )
        .unwrap();

        let b: Value = serde_yaml::from_str(r#""#).unwrap();

        merge_yaml(&mut a, &b, overwrite);

        let expected: Value = serde_yaml::from_str(
            r#"
            key: value
        "#,
        )
        .unwrap();

        assert_eq!(a, expected);
    }
}
