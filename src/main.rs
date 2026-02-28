use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::time::{Duration, Instant};

use procwarden::app::App;
use procwarden::common::DataSourceMode;
use procwarden::data_source::{self, PollingDataSource};
#[cfg(feature = "etw")]
use procwarden::data_source::EtwDataSource;
use procwarden::ui::{self, init_terminal, restore_terminal, render_ui};
use procwarden::ui::rendering::PriorityGuardStatus;

fn main() -> Result<()> {
    env_logger::init();

    // Initialize terminal
    let mut terminal = init_terminal()?;

    // Prefer ETW when available; fall back to polling if ETW is unavailable,
    // fails to initialize, or requires admin privileges.
    // Use --disable-etw to force polling mode if needed.
    let disable_etw = std::env::args().any(|arg| arg == "--disable-etw");
    let (data_source, mode): (Box<dyn data_source::DataSource>, DataSourceMode) = if cfg!(feature = "etw") && !disable_etw {
        #[cfg(feature = "etw")]
        {
            match EtwDataSource::new() {
                Ok(etw) => {
                    log::info!("Using ETW data source");
                    (Box::new(etw), DataSourceMode::Etw)
                }
                Err(e) => {
                    log::warn!("ETW initialization failed ({}), falling back to polling", e);
                    (Box::new(PollingDataSource::new()), DataSourceMode::PollingFallback)
                }
            }
        }
        #[cfg(not(feature = "etw"))]
        {
            unreachable!("ETW feature indicated but not compiled")
        }
    } else {
        log::info!("Using polling data source");
        (Box::new(PollingDataSource::new()), DataSourceMode::PollingFallback)
    };
    
    let mut app = App::new(data_source, mode);

    // Load persisted settings
    let persisted = procwarden::config::load();
    procwarden::config::apply_to(&persisted, &mut app.settings);

    // Run application
    let result = run_app(&mut terminal, &mut app);

    // Cleanup PriorityGuard (restore all demoted processes)
    app.priority_guard.cleanup();

    // Restore terminal
    restore_terminal(&mut terminal)?;

    // Handle result
    if let Err(err) = result {
        println!("Error: {:?}", err);
    }

    Ok(())
}

fn run_app(
    terminal: &mut ui::TerminalHandle,
    app: &mut App,
) -> Result<()> {
    let mut last_refresh = Instant::now() - Duration::from_secs(1); // force initial refresh
    let mut needs_render = true;

    loop {
        // Only poll process data when the refresh interval has elapsed
        let poll_interval = Duration::from_millis(app.poll_interval_ms());
        if last_refresh.elapsed() >= poll_interval {
            app.update_processes()?;
            last_refresh = Instant::now();
            needs_render = true;
        }

        if needs_render {
            let pg_status = PriorityGuardStatus {
                enabled: app.settings.priority_guard_enabled,
                active_demotions: app.priority_guard.active_demotion_count(),
                demoted_pids: app.priority_guard.demoted_pids(),
                log_entries: app.priority_guard.log_entries().iter().cloned().collect(),
            };
            terminal.draw(|f| {
                let action_state = ui::ActionState {
                    pending_action: &app.pending_action,
                    suspended_pids: &app.suspended_pids,
                    priority_picker_selected: app.priority_picker_selected,
                };
                render_ui(f, app.ui_mode, &mut app.process_list, &app.settings, app.data_source_mode, &pg_status, app.log_scroll_offset, &action_state);
            })?;
            needs_render = false;
        }

        // Wait for input or next refresh, whichever comes first
        let remaining = poll_interval.saturating_sub(last_refresh.elapsed());
        if crossterm::event::poll(remaining)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Ctrl+C quits from any mode
                    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                        return Ok(());
                    }
                    if app.on_key(key.code)? {
                        return Ok(());
                    }
                    needs_render = true;
                }
            }
        }
    }
}
