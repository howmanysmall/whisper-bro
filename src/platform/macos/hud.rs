use std::time::{Duration, Instant};

use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFont, NSPanel, NSScreen, NSStatusItem, NSStatusWindowLevel,
    NSTextField, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

const PADDING_X: f64 = 14.0;
const PADDING_Y: f64 = 8.0;
const MAX_TEXT_WIDTH: f64 = 420.0;
const GAP: f64 = 6.0;

/// A small floating panel under the status item. It never activates and ignores the mouse,
/// so the app being dictated into keeps keyboard focus.
pub struct Hud {
    panel: Retained<NSPanel>,
    label: Retained<NSTextField>,
    item: Retained<NSStatusItem>,
    until: Option<Instant>,
}

impl Hud {
    pub fn new(mtm: MainThreadMarker, item: Retained<NSStatusItem>) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(120.0, 32.0));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        // Closing must not free the panel while `Retained` still owns it.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setLevel(NSStatusWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::Transient,
        );
        panel.setIgnoresMouseEvents(true);
        panel.setHidesOnDeactivate(false);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setFloatingPanel(true);
        panel.setBecomesKeyOnlyIfNeeded(true);

        let effect = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
        effect.setMaterial(NSVisualEffectMaterial::HUDWindow);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::Active);
        effect.setWantsLayer(true);
        if let Some(layer) = effect.layer() {
            layer.setCornerRadius(10.0);
            layer.setMasksToBounds(true);
        }

        let label = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
        label.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        label.setTextColor(Some(&NSColor::labelColor()));
        label.setPreferredMaxLayoutWidth(MAX_TEXT_WIDTH);
        effect.addSubview(&label);
        panel.setContentView(Some(&effect));
        Self {
            panel,
            label,
            item,
            until: None,
        }
    }

    /// Shows `text` until `ttl` passes, or until the next `show`/`hide` when `ttl` is `None`.
    pub fn show(&mut self, text: &str, ttl: Option<Duration>) {
        self.label.setStringValue(&NSString::from_str(text));
        let text_size = match self.label.cell() {
            Some(cell) => cell.cellSizeForBounds(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(MAX_TEXT_WIDTH, 10_000.0),
            )),
            None => {
                self.label.sizeToFit();
                self.label.frame().size
            }
        };
        let text_size = NSSize::new(text_size.width.ceil(), text_size.height.ceil());
        self.label
            .setFrame(NSRect::new(NSPoint::new(PADDING_X, PADDING_Y), text_size));
        let size = NSSize::new(
            text_size.width + 2.0 * PADDING_X,
            text_size.height + 2.0 * PADDING_Y,
        );
        self.panel
            .setFrame_display(NSRect::new(self.origin(size), size), true);
        self.panel.orderFrontRegardless();
        self.until = ttl.map(|ttl| Instant::now() + ttl);
    }

    pub fn tick(&mut self) {
        if self.until.is_some_and(|until| Instant::now() >= until) {
            self.hide();
        }
    }

    pub fn hide(&mut self) {
        self.until = None;
        self.panel.orderOut(None);
    }

    /// Centres the panel under the status item, kept inside the visible screen area.
    fn origin(&self, size: NSSize) -> NSPoint {
        let mtm = self.panel.mtm();
        let screen = NSScreen::mainScreen(mtm).map(|screen| screen.visibleFrame());
        let anchor = self
            .item
            .button(mtm)
            .and_then(|button| button.window())
            .map(|window| window.frame())
            .or(screen);
        let Some(anchor) = anchor else {
            return NSPoint::new(0.0, 0.0);
        };
        let mut x = anchor.origin.x + anchor.size.width / 2.0 - size.width / 2.0;
        let mut y = anchor.origin.y - size.height - GAP;
        if let Some(visible) = screen {
            let right = visible.origin.x + visible.size.width - size.width - GAP;
            x = x.min(right).max(visible.origin.x + GAP);
            let top = visible.origin.y + visible.size.height - size.height - GAP;
            y = y.min(top).max(visible.origin.y + GAP);
        }
        NSPoint::new(x, y)
    }
}
