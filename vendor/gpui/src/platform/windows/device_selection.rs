//! Select an adapter together with the device that established its suitability.
//!
//! Keeping the result prevents an expensive second device creation and ensures
//! the renderer uses the device whose capabilities were actually checked.
pub(super) fn select_device<A, D, E>(
    adapters: impl IntoIterator<Item = Result<A, E>>,
    mut create: impl FnMut(&A) -> Option<D>,
) -> Result<Option<(A, D)>, E> {
    for adapter in adapters {
        let adapter = adapter?;
        if let Some(device) = create(&adapter) {
            return Ok(Some((adapter, device)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_the_tested_device_and_stops_at_first_supported_adapter() {
        let mut attempts = Vec::new();
        let result = select_device([Ok::<_, ()>(0), Ok(1), Ok(2)], |adapter| {
            attempts.push(*adapter);
            (*adapter == 1).then_some("tested device and context")
        });
        assert_eq!(result, Ok(Some((1, "tested device and context"))));
        assert_eq!(attempts, [0, 1]);
    }

    #[test]
    fn enumeration_failure_is_preserved_without_trying_later_adapters() {
        let mut attempts = Vec::new();
        let result = select_device([Ok(0), Err("enumeration failed"), Ok(2)], |adapter| {
            attempts.push(*adapter);
            None::<()>
        });
        assert_eq!(result, Err("enumeration failed"));
        assert_eq!(attempts, [0]);
    }

    #[test]
    fn no_supported_device_is_distinct_from_an_enumeration_error() {
        assert_eq!(select_device([Ok::<_, ()>(0)], |_| None::<()>), Ok(None));
    }
}
