//! Table of contents sidebar UI component (Milestone 5).

use egui::{Color32, RichText, ScrollArea, Ui};

use crate::pdf::types::OutlineItem;

/// Result of rendering the table of contents sidebar.
#[derive(Debug, Default)]
pub struct SidebarTocOutput {
    /// Target 1-based page number to scroll to if a bookmark was clicked.
    pub target_page: Option<usize>,
    /// Whether the user clicked the close sidebar button.
    pub close_requested: bool,
}

/// Checks whether an outline item corresponds to the active chapter for the given current_page.
pub fn is_chapter_active(
    item: &OutlineItem,
    current_page: usize,
    next_sibling_page: Option<usize>,
) -> bool {
    if let Some(start) = item.page_number {
        if current_page >= start {
            if let Some(end) = next_sibling_page {
                return current_page < end;
            }
            return true;
        }
    }
    false
}

/// Recursively renders a slice of outline items as a collapsible tree.
fn render_outline_tree(
    ui: &mut Ui,
    items: &[OutlineItem],
    current_page: usize,
    depth: usize,
) -> Option<usize> {
    let mut jump_page = None;

    for (idx, item) in items.iter().enumerate() {
        let next_sibling_page = items.get(idx + 1).and_then(|sibling| sibling.page_number);
        let is_current = is_chapter_active(item, current_page, next_sibling_page);

        let is_dark = ui.visuals().dark_mode;
        let label_color = if is_current {
            if is_dark {
                Color32::from_rgb(100, 180, 255) // Accent blue for active chapter
            } else {
                Color32::from_rgb(0, 100, 200)
            }
        } else if is_dark {
            Color32::from_rgb(220, 220, 225)
        } else {
            Color32::from_rgb(45, 50, 60)
        };

        let mut title_text = RichText::new(&item.title).color(label_color).size(12.5);
        if is_current {
            title_text = title_text.strong();
        }

        if item.children.is_empty() {
            // Leaf bookmark: simple indented button
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 12.0);
                let mut btn = ui
                    .add(
                        egui::Button::new(title_text)
                            .frame(false)
                            .wrap_mode(egui::TextWrapMode::Truncate),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);

                if let Some(p) = item.page_number {
                    btn = btn.on_hover_text(format!("Page {p}"));
                    if btn.clicked() {
                        jump_page = Some(p);
                    }
                }
            });
        } else {
            // Branch bookmark: collapsible header
            let id = ui.id().with(("outline_node", depth, idx, &item.title));
            let header = egui::CollapsingHeader::new(title_text)
                .id_salt(id)
                .default_open(depth == 0);

            let response = header.show(ui, |ui| {
                if let Some(p) = render_outline_tree(ui, &item.children, current_page, depth + 1) {
                    jump_page = Some(p);
                }
            });

            let header_resp = response
                .header_response
                .on_hover_cursor(egui::CursorIcon::PointingHand);

            if let Some(p) = item.page_number {
                if header_resp.clicked() {
                    jump_page = Some(p);
                }
                header_resp.on_hover_text(format!("Page {p}"));
            }
        }
    }

    jump_page
}

/// Renders the Table of Contents sidebar.
pub fn render_sidebar_toc(
    ui: &mut Ui,
    outline: Option<&[OutlineItem]>,
    is_loading: bool,
    current_page: usize,
) -> SidebarTocOutput {
    let mut output = SidebarTocOutput::default();

    ui.horizontal(|ui| {
        ui.label(RichText::new("Table of Contents").strong().size(13.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let close_btn = crate::ui::icons::render_icon_button(
                ui,
                egui::vec2(22.0, 22.0),
                false,
                true,
                "Close sidebar",
                crate::ui::icons::paint_close,
            );
            if close_btn.clicked() {
                output.close_requested = true;
            }
        });
    });

    ui.separator();

    let palette = crate::ui::theme::Theme::palette(ui.visuals().dark_mode);
    if is_loading {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.spinner();
            ui.add_space(8.0);
            ui.label(RichText::new("Loading outline...").color(palette.text_muted));
        });
    } else {
        match outline {
            Some(items) if !items.is_empty() => {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(p) = render_outline_tree(ui, items, current_page, 0) {
                            output.target_page = Some(p);
                        }
                    });
            }
            _ => {
                ui.vertical_centered(|ui| {
                    ui.add_space(30.0);
                    ui.label(RichText::new("No table of contents").color(palette.text_muted));
                });
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_chapter_active() {
        let item = OutlineItem {
            title: "Chapter 1".to_string(),
            page_number: Some(10),
            children: Vec::new(),
        };

        // Before chapter
        assert!(!is_chapter_active(&item, 5, Some(25)));
        // Exactly on start
        assert!(is_chapter_active(&item, 10, Some(25)));
        // Inside chapter
        assert!(is_chapter_active(&item, 18, Some(25)));
        // At or beyond next sibling
        assert!(!is_chapter_active(&item, 25, Some(25)));
        assert!(!is_chapter_active(&item, 30, Some(25)));
        // No next sibling
        assert!(is_chapter_active(&item, 100, None));
    }
}
