//! claude, the vendor amx was written against.
//!
//! Everything here is the vendor's own words, measured against 2.1.237's
//! `--help`. Re-measure at every vendor bump: these are not amx's names to
//! choose, and a renamed mode or a dropped alias turns a dial into a spawn
//! that fails.

use super::{DEFAULT, DialSpec, Vendor};

/// claude's entry in the table.
pub const VENDOR: Vendor = Vendor {
    name: "claude",
    // Open: `--help` says an alias "or a model's full name", so the cycle
    // lists the three aliases and the dial takes anything.
    model: Some(DialSpec {
        cycle: &[DEFAULT, "fable", "opus", "sonnet"],
        open: true,
        flag: "--model",
    }),
    // Closed, and the vendor enforces it: `--permission-mode nonsense` is a
    // hard error naming these six.
    permission: Some(DialSpec {
        cycle: &[
            DEFAULT,
            "acceptEdits",
            "auto",
            "bypassPermissions",
            "manual",
            "dontAsk",
            "plan",
        ],
        open: false,
        flag: "--permission-mode",
    }),
    // Closed by judgement rather than by the vendor: `--effort nonsense`
    // warns and falls back to the default rather than refusing. Five levels
    // are the whole documented set, so amx warns at the config it can see
    // instead of leaving the person to find the vendor's warning scrolled off
    // the top of a pane.
    effort: Some(DialSpec {
        cycle: &[DEFAULT, "low", "medium", "high", "xhigh", "max"],
        open: false,
        flag: "--effort",
    }),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_declares_a_model_a_permission_and_an_effort_dial() {
        // Measured against claude 2.1.237's `--help`. Re-measure at every
        // vendor bump: a renamed mode or a dropped alias turns a dial into a
        // spawn that fails.
        let model = VENDOR.model.expect("claude has a model dial");
        assert_eq!(model.flag, "--model");
        assert_eq!(model.cycle, ["default", "fable", "opus", "sonnet"]);
        assert!(model.open, "--model takes a full model name too");

        let permission = VENDOR.permission.expect("claude has a permission dial");
        assert_eq!(permission.flag, "--permission-mode");
        assert_eq!(
            permission.cycle,
            [
                "default",
                "acceptEdits",
                "auto",
                "bypassPermissions",
                "manual",
                "dontAsk",
                "plan"
            ]
        );
        assert!(!permission.open, "--permission-mode is a closed set");

        let effort = VENDOR.effort.expect("claude has an effort dial");
        assert_eq!(effort.flag, "--effort");
        assert_eq!(
            effort.cycle,
            ["default", "low", "medium", "high", "xhigh", "max"]
        );
        assert!(!effort.open, "--effort is a closed set");
    }
}
