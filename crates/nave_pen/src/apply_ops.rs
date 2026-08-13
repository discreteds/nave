//! Apply-mode mutation verbs: branch/commit/push/reset over a Nave pen,
//! plus the stateless capabilities probe. Mirrors `ops.rs`'s mutation
//! idiom; uses `git_util` (captured output) and `apply_state` (the
//! cross-invocation sidecar) instead of `ops.rs`'s fire-and-forget helpers.

use nave_apply::{APPLY_VERBS, AdapterState, CapabilitiesResult, PROTOCOL_VERSION};

pub fn capabilities() -> CapabilitiesResult {
    CapabilitiesResult {
        protocol_version: PROTOCOL_VERSION,
        verbs: APPLY_VERBS.iter().map(ToString::to_string).collect(),
        adapter_state: AdapterState::Ok,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_reports_protocol_and_verbs() {
        let caps = capabilities();
        assert_eq!(caps.protocol_version, nave_apply::PROTOCOL_VERSION);
        assert_eq!(
            caps.verbs,
            nave_apply::APPLY_VERBS
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        assert!(matches!(caps.adapter_state, nave_apply::AdapterState::Ok));
    }
}
