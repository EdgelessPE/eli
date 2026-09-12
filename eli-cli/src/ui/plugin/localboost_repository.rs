use crate::ui::window as window_support;
use eli_lib::command::plugin::localboost::repository::RepositoryCandidate;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;

slint::include_modules!();

pub(crate) fn select(candidates: Vec<RepositoryCandidate>) -> io::Result<Option<PathBuf>> {
    install_software_backend()?;
    let window = LocalBoostRepositoryWindow::new().map_err(io::Error::other)?;
    let paths = candidates
        .iter()
        .map(|candidate| candidate.mount_point.clone())
        .collect::<Vec<_>>();
    let rows = candidates
        .into_iter()
        .map(|candidate| RepositoryRow {
            mount_point: candidate.mount_point.display().to_string().into(),
            label: candidate.label.into(),
            available: format_bytes(candidate.available_bytes).into(),
            reason: candidate.reason.unwrap_or_default().into(),
            selectable: candidate.selectable,
            has_repository: candidate.has_repository,
        })
        .collect::<Vec<_>>();
    window.set_rows(ModelRc::new(VecModel::from(rows)));
    let selected = Rc::new(RefCell::new(None));
    configure_window(&window);

    let selected_for_confirm = Rc::clone(&selected);
    let paths_for_confirm = paths.clone();
    let window_weak = window.as_weak();
    window.on_confirm_requested(move |index| {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        let Some(path) = paths_for_confirm.get(index).cloned() else {
            return;
        };
        *selected_for_confirm.borrow_mut() = Some(path);
        if let Some(window) = window_weak.upgrade() {
            let _ = window.hide();
        }
    });

    let window_weak = window.as_weak();
    window.on_cancel_requested(move || {
        if let Some(window) = window_weak.upgrade() {
            let _ = window.hide();
        }
    });
    window.run().map_err(io::Error::other)?;
    let value = selected.borrow().clone();
    Ok(value)
}

fn install_software_backend() -> io::Result<()> {
    let backend = i_slint_backend_winit::Backend::builder()
        .with_renderer_name("software")
        .build()
        .map_err(|error| io::Error::other(error.to_string()))?;
    slint::platform::set_platform(Box::new(backend))
        .map_err(|error| io::Error::other(error.to_string()))
}

fn configure_window(window: &LocalBoostRepositoryWindow) {
    let window_weak = window.as_weak();
    window
        .global::<WindowAdapter>()
        .on_apply_window_region(move |corner_radius| {
            let Some(window) = window_weak.upgrade() else {
                return false;
            };
            window_support::apply_window_region(window.window(), corner_radius)
        });

    let window_weak = window.as_weak();
    window
        .global::<WindowAdapter>()
        .on_install_titlebar_hit_test(move |titlebar_height, left_inset, right_inset| {
            let Some(window) = window_weak.upgrade() else {
                return false;
            };
            window_support::install_titlebar_hit_test(
                window.window(),
                titlebar_height,
                left_inset,
                right_inset,
            )
        });
}

fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    format!("可用 {:.1} GiB", bytes as f64 / GIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_available_space_for_the_candidate_row() {
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "可用 3.0 GiB");
    }
}
