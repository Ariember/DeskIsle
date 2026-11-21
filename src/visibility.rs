#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockDisplayMode {
    Default,
    Overlay,
    AutoHide,
    SmartHide,
    Always,
    Hotkey,
    AlwaysIncludingFullscreen,
    Desktop,
}

#[derive(Clone, Copy, Default)]
pub struct DockContext {
    pub fullscreen: bool,
    pub desktop: bool,
    /// Intersection with the expanded Dock, even while it is collapsed.
    pub overlaps: bool,
    pub hotkey_shown: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockPolicy {
    Hidden,
    /// The bottom pixel can reveal the Dock after the configured dwell.
    EdgeReveal,
    Shown,
}

impl DockDisplayMode {
    pub fn policy(self, context: DockContext) -> DockPolicy {
        if context.fullscreen && self != Self::AlwaysIncludingFullscreen {
            return DockPolicy::Hidden;
        }
        match self {
            Self::AutoHide => DockPolicy::EdgeReveal,
            Self::SmartHide if context.overlaps => DockPolicy::EdgeReveal,
            Self::Hotkey if !context.hotkey_shown => DockPolicy::Hidden,
            Self::Desktop if !context.desktop => DockPolicy::Hidden,
            _ => DockPolicy::Shown,
        }
    }

    pub fn topmost(self) -> bool {
        self != Self::Overlay
    }

    pub fn reassert_topmost(self) -> bool {
        matches!(self, Self::Always | Self::AlwaysIncludingFullscreen)
    }
}

#[cfg(test)]
mod tests {
    use super::{DockContext, DockDisplayMode::*, DockPolicy::*};

    #[test]
    fn fullscreen_overrides_the_desktop_hotkey_and_edge() {
        let fullscreen = DockContext {
            fullscreen: true,
            desktop: false,
            overlaps: true,
            hotkey_shown: true,
        };
        for mode in [
            Default, Overlay, AutoHide, SmartHide, Always, Hotkey, Desktop,
        ] {
            assert_eq!(mode.policy(fullscreen), Hidden, "{mode:?}");
        }
        assert_eq!(AlwaysIncludingFullscreen.policy(fullscreen), Shown);
    }

    #[test]
    fn edge_reveal_follows_the_selected_mode_and_foreground_overlap() {
        let clear = DockContext::default();
        let covered = DockContext {
            overlaps: true,
            ..clear
        };
        assert_eq!(AutoHide.policy(clear), EdgeReveal);
        assert_eq!(AutoHide.policy(covered), EdgeReveal);
        assert_eq!(SmartHide.policy(clear), Shown);
        assert_eq!(SmartHide.policy(covered), EdgeReveal);
        for mode in [Default, Overlay, Always, AlwaysIncludingFullscreen] {
            assert_eq!(mode.policy(covered), Shown, "{mode:?}");
        }
    }

    #[test]
    fn hotkey_and_desktop_modes_have_independent_reveal_conditions() {
        let desktop = DockContext {
            desktop: true,
            ..DockContext::default()
        };
        assert_eq!(Desktop.policy(desktop), Shown);
        assert_eq!(Hotkey.policy(desktop), Hidden);
        let application = DockContext {
            hotkey_shown: true,
            ..DockContext::default()
        };
        assert_eq!(Desktop.policy(application), Hidden);
        assert_eq!(Hotkey.policy(application), Shown);
    }

    #[test]
    fn overlay_can_be_covered_and_always_modes_reclaim_the_top() {
        assert!(!Overlay.topmost());
        assert!(Default.topmost());
        assert!(!Default.reassert_topmost());
        for mode in [Always, AlwaysIncludingFullscreen] {
            assert!(mode.topmost());
            assert!(mode.reassert_topmost());
        }
    }
}
