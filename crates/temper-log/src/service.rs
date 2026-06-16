// SPDX-License-Identifier: MPL-2.0

//! The `service` dimension: which plane of the daemon produced an event.
//!
//! Every structured event names the subsystem that emitted it (§2 of the
//! logging design). A [`Service`] maps 1:1 onto:
//!
//! - a stable lowercase string for the `service=` machine field
//!   ([`Service::as_str`]),
//! - a [`tracing`] `target` so `RUST_LOG=temper::worker=debug` filters one plane
//!   ([`Service::target`]),
//! - a fixed-width human prefix (`engine:  `, `worker:  `, `agent:   `,
//!   `trigger: `) so the message column aligns in `journalctl` output
//!   ([`Service::human_prefix`]).
//!
//! The `target` values are `'static` string literals because [`tracing`] macros
//! require a literal (or constant) `target:` argument; [`Service::target`]
//! therefore returns `&'static str` chosen by a match rather than a formatted
//! string.

/// The subsystem ("plane") that produced an event.
///
/// These map onto the standalone daemon's assembly (engine + worker + agent on
/// one loop) plus the inbound trigger. See §2 of the logging design.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Service {
    /// Orchestrator: applied transitions, label diffs, queue moves, gate
    /// evaluation, merges, resolution. The authoritative state-change log.
    Engine,
    /// Lease lifecycle and per-role concurrency: claims, saturation, releases.
    Worker,
    /// LLM workspace runs (triage, coding): start/finish with duration + result.
    Agent,
    /// Forge webhook/wake receiver: inbound facts from the outside world.
    Trigger,
}

impl Service {
    /// The stable lowercase token used for the `service=` machine field.
    ///
    /// This is the value an agent filters on (`where service == "engine"`); it
    /// never changes once shipped.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Engine => "engine",
            Self::Worker => "worker",
            Self::Agent => "agent",
            Self::Trigger => "trigger",
        }
    }

    /// The [`tracing`] target for this service (`temper::engine`, …).
    ///
    /// Used as the `target:` argument of the emit macros so `RUST_LOG` can
    /// filter one plane: `RUST_LOG=temper::worker=debug,info`.
    pub const fn target(self) -> &'static str {
        match self {
            Self::Engine => "temper::engine",
            Self::Worker => "temper::worker",
            Self::Agent => "temper::agent",
            Self::Trigger => "temper::trigger",
        }
    }

    /// The fixed-width human prefix, e.g. `"engine:  "` or `"trigger: "`.
    ///
    /// Padded so the message column aligns across services in the journal
    /// (`agent:   ` is the longest token plus a colon; every prefix is padded to
    /// that width). The trailing space(s) are part of the returned string, so a
    /// human line is `format!("{}{body}", service.human_prefix())`.
    pub const fn human_prefix(self) -> &'static str {
        // The widest token is "trigger" (7 chars). With a trailing ": " the
        // prefix column is 9 chars wide; the shorter tokens pad to match.
        match self {
            Self::Engine => "engine:  ",
            Self::Worker => "worker:  ",
            Self::Agent => "agent:   ",
            Self::Trigger => "trigger: ",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_is_stable_lowercase() {
        assert_eq!(Service::Engine.as_str(), "engine");
        assert_eq!(Service::Worker.as_str(), "worker");
        assert_eq!(Service::Agent.as_str(), "agent");
        assert_eq!(Service::Trigger.as_str(), "trigger");
    }

    #[test]
    fn target_is_dotted_temper_namespace() {
        assert_eq!(Service::Engine.target(), "temper::engine");
        assert_eq!(Service::Worker.target(), "temper::worker");
        assert_eq!(Service::Agent.target(), "temper::agent");
        assert_eq!(Service::Trigger.target(), "temper::trigger");
    }

    #[test]
    fn human_prefixes_are_padded_to_equal_width() {
        // The §2 alignment rule: every prefix is the same display width so the
        // message column lines up in `journalctl` output.
        let width = Service::Trigger.human_prefix().len();
        for service in [
            Service::Engine,
            Service::Worker,
            Service::Agent,
            Service::Trigger,
        ] {
            assert_eq!(
                service.human_prefix().len(),
                width,
                "{service:?} prefix is not padded to the common width"
            );
            assert!(
                service.human_prefix().starts_with(service.as_str()),
                "{service:?} prefix must start with its token"
            );
        }
        // Spot-check the exact strings from §7's reference output.
        assert_eq!(Service::Engine.human_prefix(), "engine:  ");
        assert_eq!(Service::Trigger.human_prefix(), "trigger: ");
    }
}
