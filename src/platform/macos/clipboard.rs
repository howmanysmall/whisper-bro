use std::time::Duration;

use anyhow::{Result, ensure};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString};

use super::accessibility;

pub fn copy_text(text: &str) -> Result<()> {
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    ensure!(
        unsafe { board.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) },
        "Could not write to the clipboard"
    );
    Ok(())
}

/// Writes the dictation marked transient and concealed (nspasteboard.org), so clipboard managers skip it.
fn write_transient(board: &NSPasteboard, text: &str) -> Result<()> {
    let item = NSPasteboardItem::new();
    ensure!(
        item.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString }),
        "Could not write to the clipboard"
    );
    let marker = NSData::new();
    for kind in [
        "org.nspasteboard.TransientType",
        "org.nspasteboard.ConcealedType",
    ] {
        ensure!(
            item.setData_forType(&marker, &NSString::from_str(kind)),
            "Could not write to the clipboard"
        );
    }
    board.clearContents();
    ensure!(
        board.writeObjects(&NSArray::from_retained_slice(&[
            ProtocolObject::from_retained(item)
        ])),
        "Could not write to the clipboard"
    );
    Ok(())
}

pub fn paste(
    text: &str,
    restore_after: Duration,
    check_target: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let board = NSPasteboard::generalPasteboard();
    let initial_change = board.changeCount();
    let mut snapshot: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = Vec::new();
    if let Some(items) = board.pasteboardItems() {
        for item in items {
            let copy = NSPasteboardItem::new();
            for kind in item.types() {
                let Some(data) = item.dataForType(&kind) else {
                    anyhow::bail!("Could not preserve an existing clipboard format");
                };
                ensure!(
                    copy.setData_forType(&data, &kind),
                    "Could not preserve the clipboard"
                );
            }
            snapshot.push(ProtocolObject::from_retained(copy));
        }
    }
    ensure!(
        board.changeCount() == initial_change,
        "The clipboard changed while preparing to paste"
    );
    write_transient(&board, text)?;
    let our_change = board.changeCount();
    let inserted = check_target().and_then(|()| accessibility::paste_key());
    std::thread::sleep(restore_after);
    if board.changeCount() == our_change {
        board.clearContents();
        if !snapshot.is_empty() && !board.writeObjects(&NSArray::from_retained_slice(&snapshot)) {
            tracing::warn!("Text was inserted, but the previous clipboard could not be restored");
        }
    }
    inserted
}
