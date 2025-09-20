use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;
use std::time::Instant;

use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::Global;
use slint::LogicalSize;
use slint::Model;
use slint::ModelRc;
use slint::SharedString;
use slint::TableColumn;
use slint::ToSharedString;
use slint::VecModel;
use slint::Weak;
use slint::quit_event_loop;
use slint::spawn_local;
use strum::EnumString;
use strum::IntoEnumIterator;
use tracing::debug;
use tracing::debug_span;
use tracing::info;
use tracing::info_span;

use crate::action::Action;
use crate::appstate::AppState;
use crate::backend::BackendRuntime;
use crate::backend::ChildWindow;
use crate::backend::WindowExt as _;
use crate::backend::WinitAccelerator;
use crate::channel::Channel;
use crate::collections::add_items_to_existing_folder_collection;
use crate::collections::add_items_to_new_folder_collection;
use crate::collections::get_collection_name;
use crate::collections::get_folder_collection_names;
use crate::collections::get_folder_collections;
use crate::collections::remove_items_from_folder_collection;
use crate::collections::toggle_builtin_collection;
use crate::devimageconfig::DevicesImagesConfig;
use crate::dialogs::cheats::dialog_cheats;
use crate::dialogs::configure::dialog_configure;
use crate::dialogs::devimages::dialog_devices_and_images;
use crate::dialogs::file::initial_dir_and_file_from_path;
use crate::dialogs::file::load_file_dialog;
use crate::dialogs::file::save_file_dialog;
use crate::dialogs::image::Format;
use crate::dialogs::image::dialog_load_image;
use crate::dialogs::importmameini::dialog_import_mame_ini;
use crate::dialogs::input::multi::dialog_input_select_multiple;
use crate::dialogs::input::primary::dialog_input;
use crate::dialogs::input::xy::dialog_input_xy;
use crate::dialogs::messagebox::OkCancel;
use crate::dialogs::messagebox::OkOnly;
use crate::dialogs::messagebox::dialog_message_box;
use crate::dialogs::namecollection::dialog_new_collection;
use crate::dialogs::namecollection::dialog_rename_collection;
use crate::dialogs::paths::dialog_paths;
use crate::dialogs::seqpoll::dialog_seq_poll;
use crate::dialogs::socket::dialog_connect_to_socket;
use crate::dialogs::switches::dialog_switches;
use crate::guiutils::is_context_menu_event;
use crate::guiutils::modal::ModalStack;
use crate::history::History;
use crate::models::collectionsview::CollectionsViewModel;
use crate::models::itemstable::EmptyReason;
use crate::models::itemstable::ItemsTableModel;
use crate::prefs::BuiltinCollection;
use crate::prefs::Preferences;
use crate::prefs::PrefsCollection;
use crate::prefs::SortOrder;
use crate::prefs::pathtype::PathType;
use crate::runtime::MameStderr;
use crate::runtime::MameWindowing;
use crate::runtime::command::MameCommand;
use crate::runtime::command::MovieFormat;
use crate::selection::SelectionManager;
use crate::snapview::SnapView;
use crate::status::InputClass;
use crate::status::Status;
use crate::ui::AboutDialog;
use crate::ui::AppWindow;
use crate::ui::Icons;
use crate::ui::ReportIssue;
use crate::ui::SimpleMenuEntry;
use crate::version::MameVersion;

const SOUND_ATTENUATION_OFF: i32 = -32;
const SOUND_ATTENUATION_ON: i32 = 0;

const SAVE_STATE_EXTENSION: &str = "sta";
const SAVE_STATE_FILE_TYPES: &[(Option<&str>, &str)] = &[(Some("MAME Saved State Files"), SAVE_STATE_EXTENSION)];

const THROTTLE_RATES: &[f32] = &[10.0, 5.0, 2.0, 1.0, 0.5, 0.2, 0.1];
const FRAMESKIP_RATES: &[Option<u8>] = &[
	None,
	Some(0),
	Some(1),
	Some(2),
	Some(3),
	Some(4),
	Some(5),
	Some(6),
	Some(7),
	Some(8),
	Some(9),
	Some(10),
];

const MINIMUM_MAME_RECORD_MOVIE: MameVersion = MameVersion::new(0, 221); // recording movies by specifying absolute paths was introduced in MAME 0.221
const MINIMUM_MAME_CLASSIC_MENU: MameVersion = MameVersion::new(0, 274);

/// Arguments to the application (derivative from the command line); almost all of this
/// are power user features or diagnostics
pub struct AppArgs {
	pub prefs_path: PathBuf,
	pub mame_stderr: MameStderr,
	pub mame_windowing: AppWindowing,
	pub backend_runtime: BackendRuntime,
}

#[derive(Debug, Default, EnumString)]
pub enum AppWindowing {
	#[default]
	#[strum(ascii_case_insensitive)]
	Integrated,
	#[strum(ascii_case_insensitive)]
	Windowed,
	#[strum(ascii_case_insensitive)]
	WindowedMaximized,
	#[strum(ascii_case_insensitive)]
	Fullscreen,
}

struct AppModel {
	app_window_weak: Weak<AppWindow>,
	backend_runtime: BackendRuntime,
	modal_stack: ModalStack,
	preferences: RefCell<Preferences>,
	state: RefCell<AppState>,
	status_changed_channel: Channel<Status>,
	child_window: RefCell<Option<ChildWindow>>,
	snap_image: SnapView,
}

impl AppModel {
	pub fn app_window(&self) -> AppWindow {
		self.app_window_weak.unwrap()
	}

	pub fn with_collections_view_model<T>(&self, func: impl FnOnce(&CollectionsViewModel) -> T) -> T {
		let collections_model = self.app_window().get_collections_model();
		let collections_model = collections_model
			.as_any()
			.downcast_ref::<CollectionsViewModel>()
			.expect("with_collections_view_model(): downcast_ref::<CollectionsViewModel>() failed");
		func(collections_model)
	}

	pub fn with_items_table_model<T>(&self, func: impl FnOnce(&ItemsTableModel) -> T) -> T {
		let items_model = self.app_window().get_items_model();
		let items_model = items_model
			.as_any()
			.downcast_ref::<ItemsTableModel>()
			.expect("with_items_table_model(): downcast_ref::<ItemsTableModel>() failed");
		func(items_model)
	}

	pub fn modify_prefs(self: &Rc<Self>, func: impl FnOnce(&mut Preferences)) {
		// modify actual preferences, and while we're at it get the old prefs for comparison
		// purposes
		let old_prefs = {
			let mut prefs = self.preferences.borrow_mut();
			let old_prefs = prefs.clone();
			func(&mut prefs);
			old_prefs
		};

		// we need to treat the initial modify_prefs() differently; if this is the first
		// time we didn't really have "old prefs"; we determine this by checking the history
		let old_prefs = (!old_prefs.history.is_empty()).then_some(&old_prefs);

		// reborrow prefs (but not mutably)
		let prefs = self.preferences.borrow();

		// save (ignore errors)
		let _ = self.preferences.borrow().save(self.state.borrow().prefs_path());

		// update the state (unless we're new)
		if old_prefs.is_some() {
			self.update_state(|state| state.update_paths(&prefs.paths));
		}

		// react to all of the possible changes
		if old_prefs.is_none_or(|old_prefs| prefs.collections != old_prefs.collections) {
			info!("modify_prefs(): prefs.collection changed");
			let info_db = self.state.borrow().info_db().cloned();
			self.with_collections_view_model(|x| x.update(info_db, &prefs.collections));
			update_builtin_collections_menu_checked(self, &prefs);
		}

		// update the snap view paths?
		if old_prefs.is_none_or(|old_prefs| prefs.paths.snapshots != old_prefs.paths.snapshots) {
			info!("modify_prefs(): prefs.paths.snapshots changed");
			self.snap_image.set_paths(Some(&prefs.paths.snapshots));
		}

		let must_update_for_current_history_item =
			old_prefs.is_none_or(|old_prefs| prefs.current_history_entry() != old_prefs.current_history_entry());
		let must_update_items_columns_model = old_prefs.is_none_or(|old_prefs| {
			must_update_for_current_history_item || prefs.items_columns != old_prefs.items_columns
		});

		if must_update_for_current_history_item {
			info!("modify_prefs(): updating for current history item");
			update_ui_for_current_history_item(self);
		}
		if must_update_items_columns_model {
			info!("modify_prefs(): updating items column model");
			update_items_columns_model(self);
		}
		update_items_model_for_current_prefs(self);
	}

	pub fn update_state(self: &Rc<Self>, callback: impl FnOnce(&AppState) -> Option<AppState>) {
		let info_db_changed = {
			// invoke the callback to get the new state
			let mut state = self.state.borrow_mut();
			let Some(new_state) = callback(&state) else { return };

			// did the InfoDB change?
			let info_db_changed = state.info_db().is_some() != new_state.info_db().is_some()
				|| Option::zip(state.info_db().as_ref(), new_state.info_db().as_ref())
					.is_some_and(|(old, new)| !Rc::ptr_eq(old, new));

			// commit the state and return the changes
			*state = new_state;
			info_db_changed
		};

		// InfoDb changed?
		if info_db_changed {
			let info_db = self.state.borrow().info_db().cloned();
			self.with_items_table_model(|items_model| {
				let info_db = info_db.clone();
				items_model.update(Some(info_db), None, None, None, None, None, None);
			});
			self.with_collections_view_model(|collections_model| {
				let prefs = self.preferences.borrow();
				let info_db = info_db.clone();
				collections_model.update(info_db, &prefs.collections);
			});
		}

		// shutting down?
		let is_shutdown = self.state.borrow().is_shutdown();
		if is_shutdown {
			update_prefs(self);
			quit_event_loop().unwrap()
		}

		{
			let state = self.state.borrow();
			let status = state.status();
			let running = status.and_then(|s| s.running.as_ref());
			let report = state.report();
			let app_window = self.app_window();

			// status changed channel - used by the "Devices & Images" dialog
			if let Some(status) = status {
				self.status_changed_channel.publish(status);
			}

			// running machine description
			app_window.set_running_machine_desc(state.running_machine_description().into());

			// child window visibility
			if let Some(child_window) = &*self.child_window.borrow() {
				child_window.set_active(running.is_some());
			}

			// report view
			app_window.set_report_message(
				report
					.as_ref()
					.map(|r| SharedString::from(r.message.as_ref()))
					.unwrap_or_default(),
			);
			app_window.set_report_submessage(
				report
					.as_ref()
					.map(|r| SharedString::from(r.submessage.as_deref().unwrap_or_default()))
					.unwrap_or_default(),
			);
			app_window.set_report_spinning(report.as_ref().map(|r| r.is_spinning).unwrap_or_default());
			app_window.set_report_button_text(
				report
					.as_ref()
					.and_then(|r| r.button.as_ref())
					.map(|b| SharedString::from(b.text.as_ref()))
					.unwrap_or_default(),
			);
			let issues = report
				.map(|r| r.issues)
				.unwrap_or_default()
				.iter()
				.map(|issue| {
					let text = SharedString::from(issue.text.as_ref());
					let button_text =
						SharedString::from(issue.button.as_ref().map(|b| b.text.as_ref()).unwrap_or_default());
					ReportIssue { text, button_text }
				})
				.collect::<Vec<_>>();
			let issues = VecModel::from(issues);
			let issues = ModelRc::new(issues);
			app_window.set_report_issues(issues);
		}

		// menus
		update_menus(self);
	}

	pub fn issue_command(&self, command: MameCommand) {
		self.state.borrow().issue_command(command);
	}

	pub fn suggested_initial_save_filename(&self, extension: &str) -> Option<String> {
		self.state
			.borrow()
			.status()
			.as_ref()
			.and_then(|s| s.running.as_ref())
			.map(|r| format!("{}.{}", r.machine_name, extension))
	}
}

pub async fn start(app_window: &AppWindow, args: AppArgs) {
	// wait for app_window to be ready
	args.backend_runtime
		.wait_for_window_ready(app_window.window())
		.await
		.unwrap();

	// get preferences
	let prefs_path = args.prefs_path;
	let preferences = Preferences::load(&prefs_path)
		.ok()
		.flatten()
		.unwrap_or_else(|| Preferences::fresh(prefs_path.to_str().map(|x| x.into())));

	// update window preferences
	if let Some(window_size) = &preferences.window_size {
		let physical_size = LogicalSize::from(*window_size).to_physical(app_window.window().scale_factor());
		app_window.window().set_size(physical_size);
	}

	// create the window stack
	let modal_stack = ModalStack::new(args.backend_runtime.clone(), app_window);

	// create the SnapImage
	let app_window_weak = app_window.as_weak();
	let snap_image = SnapView::new(move |svci| {
		if let Some(app_window) = app_window_weak.upgrade()
			&& let Some(snap) = svci.snap
		{
			app_window.set_snap_image(snap.unwrap_or_else(|| Icons::get(&app_window).get_bletchmame()));
		}
	});

	// create the model
	let model = AppModel {
		app_window_weak: app_window.as_weak(),
		backend_runtime: args.backend_runtime,
		modal_stack,
		preferences: RefCell::new(Preferences::default()),
		state: RefCell::new(AppState::bogus()),
		status_changed_channel: Channel::default(),
		child_window: RefCell::new(None),
		snap_image,
	};
	let model = Rc::new(model);

	// set full screen
	app_window
		.window()
		.set_fullscreen_with_display(preferences.is_fullscreen, preferences.fullscreen_display.as_deref());

	// attach the menu bar (either natively or with an approximation using Slint); looking forward to Slint having first class menuing support
	let model_clone = model.clone();
	app_window.on_menu_item_action(move |command_string| {
		if let Some(command) = Action::decode_from_slint(command_string) {
			handle_action(&model_clone, command);
		}
	});

	// create a repeating future that will update the child window forever
	let model_weak = Rc::downgrade(&model);
	app_window.on_size_changed(move || {
		if let Some(model) = model_weak.upgrade().as_deref()
			&& let Some(child_window) = model.child_window.borrow().as_ref()
		{
			// set the child window size
			let top = model.app_window().invoke_menubar_height();
			child_window.update_bounds(model.app_window().window(), top);
		}
	});

	// set up the accelerator map
	let accelerator_command_map = [
		("Pause", Action::FilePause),
		("F7", Action::FileQuickLoadState),
		("Shift+F7", Action::FileQuickLoadState),
		("Ctrl+F7", Action::FileLoadState),
		("Ctrl+Shift+F7", Action::FileLoadState),
		("F12", Action::FileSaveScreenshot),
		("Shift+F12", Action::FileRecordMovie),
		("Ctrl+Alt+X", Action::FileExit),
		("F9", Action::OptionsThrottleSpeedIncrease),
		("F8", Action::OptionsThrottleSpeedDecrease),
		("F10", Action::OptionsToggleWarp),
		("F11", Action::OptionsToggleFullScreen),
		("ScrLk", Action::OptionsToggleMenuBar),
	];
	let accelerator_command_map = HashMap::<WinitAccelerator, Action>::from_iter(
		accelerator_command_map.into_iter().map(|(accelerator, command)| {
			let accelerator = WinitAccelerator::from_str(accelerator).unwrap();
			(accelerator, command)
		}),
	);
	let model_clone = model.clone();
	model
		.backend_runtime
		.install_muda_accelerator_handler(app_window.window(), move |accelerator| {
			let command = accelerator_command_map.get(accelerator);
			if let Some(command) = command {
				handle_action(&model_clone, command.clone());
			}
			command.is_some()
		});

	// set up the collections view model
	let collections_view_model = CollectionsViewModel::new(app_window.as_weak());
	let collections_view_model = Rc::new(collections_view_model);
	app_window.set_collections_model(ModelRc::new(collections_view_model.clone()));

	// set up items view model
	let selection = SelectionManager::new(
		app_window,
		AppWindow::get_items_view_selected_index,
		AppWindow::invoke_items_view_select,
	);
	let model_clone = model.clone();
	let empty_callback = move |empty_reason| {
		update_empty_reason(&model_clone, empty_reason);
	};
	let items_model = ItemsTableModel::new(selection, empty_callback);
	let items_model_clone = items_model.clone();
	app_window.set_items_model(ModelRc::new(items_model_clone));

	// bind collection selection changes to the items view model
	let collections_view_model_clone = collections_view_model.clone();
	let model_clone = model.clone();
	app_window.on_collections_view_selected(move |index| {
		let index = index.try_into().unwrap();
		if let Some(collection) = collections_view_model_clone.get(index) {
			let collection = Rc::unwrap_or_clone(collection);
			let command = Action::Browse(collection);
			handle_action(&model_clone, command);
		}
	});

	// set up back/foward buttons
	let model_clone = model.clone();
	app_window.on_history_advance_clicked(move |delta| {
		let delta = delta.try_into().unwrap();
		handle_action(&model_clone, Action::HistoryAdvance(delta));
	});

	// set up bookmark collection button
	let model_clone = model.clone();
	app_window.on_bookmark_collection_clicked(move || {
		handle_action(&model_clone, Action::BookmarkCurrentCollection);
	});

	// set up items columns
	let items_columns = preferences
		.items_columns
		.iter()
		.map(|column| {
			let mut table_column = TableColumn::default();
			table_column.title = format!("{}", column.column_type).into();
			table_column.horizontal_stretch = 1.0;
			table_column.width = column.width;
			table_column
		})
		.collect::<Vec<_>>();
	let items_columns = VecModel::from(items_columns);
	let items_columns = Rc::new(items_columns);
	let items_columns = ModelRc::from(items_columns);
	app_window.set_items_columns(items_columns);

	// set up items filter
	let model_clone = model.clone();
	app_window.on_items_sort_ascending(move |column| {
		items_set_sorting(&model_clone, column, SortOrder::Ascending);
	});
	let model_clone = model.clone();
	app_window.on_items_sort_descending(move |column| {
		items_set_sorting(&model_clone, column, SortOrder::Descending);
	});
	let model_clone = model.clone();
	app_window.on_items_search_text_changed(move |search| {
		let command = Action::SearchText(search.into());
		handle_action(&model_clone, command);
	});
	app_window.set_items_search_text(preferences.current_history_entry().search.to_shared_string());
	let model_clone = model.clone();
	app_window.on_items_current_row_changed(move || {
		let command = Action::ItemsSelectedChanged;
		handle_action(&model_clone, command);
	});

	// for when we shut down
	let model_clone = model.clone();
	app_window.window().on_close_requested(move || {
		let command = Action::FileExit;
		handle_action(&model_clone, command);
		CloseRequestResponse::KeepWindowShown
	});

	// collections popup menus
	let model_clone = model.clone();
	app_window.on_collections_row_pointer_event(move |index, evt, position| {
		if is_context_menu_event(&evt) {
			let index = usize::try_from(index).ok();
			if let Some(context_commands) = model_clone.with_collections_view_model(|x| x.context_commands(index)) {
				model_clone
					.app_window()
					.invoke_show_collection_context_menu(context_commands, position);
			}
		}
	});

	// items popup menus
	let model_clone = model.clone();
	app_window.on_items_row_pointer_event(move |index, evt, position| {
		if is_context_menu_event(&evt) {
			let index = usize::try_from(index).unwrap();
			let folder_info = get_folder_collections(&model_clone.preferences.borrow().collections);
			let has_mame_initialized = model_clone.state.borrow().status().is_some();
			if let Some(context_commands) =
				model_clone.with_items_table_model(|x| x.context_commands(index, &folder_info, has_mame_initialized))
			{
				model_clone
					.app_window()
					.invoke_show_item_context_menu(context_commands, position);
			}
		}
	});

	// report button
	let model_clone = model.clone();
	app_window.on_report_button_clicked(move || {
		let command = {
			let state = model_clone.state.borrow();
			state.report().unwrap().button.unwrap().command
		};
		handle_action(&model_clone, command);
	});

	// issue button
	let model_clone = model.clone();
	app_window.on_issue_button_clicked(move |index| {
		let index = usize::try_from(index).unwrap();
		let command = {
			let state = model_clone.state.borrow();
			let issue = state.report().unwrap().issues.into_iter().nth(index).unwrap();
			issue.button.unwrap().command
		};
		handle_action(&model_clone, command);
	});

	// throttle menu
	let menu_entries_throttle = THROTTLE_RATES
		.iter()
		.map(|&rate| {
			let title = format!("{}%", (rate * 100.0) as u32).into();
			let action = Action::OptionsThrottleRate(rate).encode_for_slint();
			SimpleMenuEntry { title, action }
		})
		.collect::<Vec<_>>();
	let menu_entries_throttle = VecModel::from(menu_entries_throttle);
	let menu_entries_throttle = ModelRc::new(menu_entries_throttle);
	app_window.set_menu_entries_throttle(menu_entries_throttle);

	// frameskip menu
	let menu_entries_frameskip = FRAMESKIP_RATES
		.iter()
		.map(|&rate| {
			let title = match rate {
				None => "Auto".to_shared_string(),
				Some(n) => format!("{}", n).into(),
			};
			let action = Action::OptionsFrameskip(rate).encode_for_slint();
			SimpleMenuEntry { title, action }
		})
		.collect::<Vec<_>>();
	let menu_entries_frameskip = VecModel::from(menu_entries_frameskip);
	let menu_entries_frameskip = ModelRc::new(menu_entries_frameskip);
	app_window.set_menu_entries_frameskip(menu_entries_frameskip);

	// builtin collections menu
	let menu_entries_builtin_collections = BuiltinCollection::iter()
		.map(|b| {
			let title = b.to_shared_string();
			let action = Action::SettingsToggleBuiltinCollection(b).encode_for_slint();
			SimpleMenuEntry { title, action }
		})
		.collect::<Vec<_>>();
	let menu_entries_builtin_collections = VecModel::from(menu_entries_builtin_collections);
	let menu_entries_builtin_collections = ModelRc::new(menu_entries_builtin_collections);
	app_window.set_menu_entries_builtin_collections(menu_entries_builtin_collections);

	// menu commands
	{
		use Action::*;
		app_window.set_menu_action_file_stop(FileStop.encode_for_slint());
		app_window.set_menu_action_file_pause(FilePause.encode_for_slint());
		app_window.set_menu_action_file_devices_and_images(FileDevicesAndImages.encode_for_slint());
		app_window.set_menu_action_file_quick_load_state(FileQuickLoadState.encode_for_slint());
		app_window.set_menu_action_file_quick_save_state(FileQuickSaveState.encode_for_slint());
		app_window.set_menu_action_file_load_state(FileLoadState.encode_for_slint());
		app_window.set_menu_action_file_save_state(FileSaveState.encode_for_slint());
		app_window.set_menu_action_file_save_screenshot(FileSaveScreenshot.encode_for_slint());
		app_window.set_menu_action_file_record_movie(FileRecordMovie.encode_for_slint());
		app_window.set_menu_action_file_debugger(FileDebugger.encode_for_slint());
		app_window.set_menu_action_file_reset_soft(FileResetSoft.encode_for_slint());
		app_window.set_menu_action_file_reset_hard(FileResetHard.encode_for_slint());
		app_window.set_menu_action_file_exit(FileExit.encode_for_slint());
		app_window.set_menu_action_options_throttle_speed_increase(OptionsThrottleSpeedIncrease.encode_for_slint());
		app_window.set_menu_action_options_throttle_speed_decrease(OptionsThrottleSpeedDecrease.encode_for_slint());
		app_window.set_menu_action_options_toggle_warp(OptionsToggleWarp.encode_for_slint());
		app_window.set_menu_action_options_toggle_fullscreen(OptionsToggleFullScreen.encode_for_slint());
		app_window.set_menu_action_options_toggle_menubar(OptionsToggleMenuBar.encode_for_slint());
		app_window.set_menu_action_options_toggle_sound(OptionsToggleSound.encode_for_slint());
		app_window.set_menu_action_options_cheats(OptionsCheats.encode_for_slint());
		app_window.set_menu_action_options_classic(OptionsClassic.encode_for_slint());
		app_window.set_menu_action_options_console(OptionsConsole.encode_for_slint());
		app_window.set_menu_action_settings_input_controller(SettingsInput(InputClass::Controller).encode_for_slint());
		app_window.set_menu_action_settings_input_keyboard(SettingsInput(InputClass::Keyboard).encode_for_slint());
		app_window.set_menu_action_settings_input_misc(SettingsInput(InputClass::Misc).encode_for_slint());
		app_window.set_menu_action_settings_input_config(SettingsInput(InputClass::Config).encode_for_slint());
		app_window.set_menu_action_settings_input_dipswitch(SettingsInput(InputClass::DipSwitch).encode_for_slint());
		app_window.set_menu_action_settings_paths(SettingsPaths(None).encode_for_slint());
		app_window.set_menu_action_settings_reset(SettingsReset.encode_for_slint());
		app_window.set_menu_action_settings_import_mame_ini(SettingsImportMameIni.encode_for_slint());
		app_window.set_menu_action_help_refresh_info_db(HelpRefreshInfoDb.encode_for_slint());
		app_window.set_menu_action_help_website(HelpWebSite.encode_for_slint());
		app_window.set_menu_action_help_about(HelpAbout.encode_for_slint());
	}

	// initial updates
	model.modify_prefs(|prefs| *prefs = preferences);

	// create the child window
	let mame_windowing = match args.mame_windowing {
		AppWindowing::Integrated => {
			let parent = app_window.window();
			let child_window = model.backend_runtime.create_child_window(parent).await.unwrap();
			let child_window_text = child_window.text();
			model.child_window.replace(Some(child_window));
			MameWindowing::Attached(child_window_text.into())
		}
		AppWindowing::Windowed => MameWindowing::Windowed,
		AppWindowing::WindowedMaximized => MameWindowing::WindowedMaximized,
		AppWindowing::Fullscreen => MameWindowing::Fullscreen,
	};

	// now create the "real initial" state, now that we have a model to work with
	let paths = model.preferences.borrow().paths.clone();
	let model_weak = Rc::downgrade(&model);
	let state = AppState::new(prefs_path, paths, mame_windowing, args.mame_stderr, move |command| {
		let model = model_weak.upgrade().unwrap();
		handle_action(&model, command);
	});

	// and lets do something with that state; specifically
	// - load the InfoDB (if availble)
	// - start the MAME session (and maybe an InfoDB build in parallel)
	model.update_state(|_| state.activate());

	// and we've started
	app_window.set_has_started(true);

	// and show the window and we're done!
	app_window.show().unwrap();
}

fn handle_action(model: &Rc<AppModel>, command: Action) {
	// tracing
	let command_str: &'static str = (&command).into();
	let span = if command.is_frequent() {
		debug_span!("handle_action", command = command_str)
	} else {
		info_span!("handle_action", command = command_str)
	};
	let _guard = span.enter();
	info!(command=?&command, "handle_action()");
	let start_instant = Instant::now();

	match command {
		Action::FileStop => {
			model.issue_command(MameCommand::stop());
		}
		Action::FilePause => {
			let is_paused = model
				.state
				.borrow()
				.status()
				.and_then(|s| s.running.as_ref())
				.map(|r| r.is_paused)
				.unwrap_or_default();
			if is_paused {
				model.issue_command(MameCommand::resume());
			} else {
				model.issue_command(MameCommand::pause());
			}
		}
		Action::FileDevicesAndImages => {
			let info_db = model.state.borrow().info_db().cloned().unwrap();
			let diconfig = DevicesImagesConfig::new(info_db);
			let diconfig = diconfig.update_status(model.state.borrow().status().as_ref().unwrap());
			let status_update_channel = model.status_changed_channel.clone();
			let model_clone = model.clone();
			let invoke_command = move |command| handle_action(&model_clone, command);
			let fut = dialog_devices_and_images(
				model.modal_stack.clone(),
				diconfig,
				status_update_channel,
				invoke_command,
			);
			spawn_local(fut).unwrap();
		}
		Action::FileQuickLoadState => {
			let last_save_state = model.state.borrow().last_save_state().unwrap();
			model.issue_command(MameCommand::state_load(last_save_state));
		}
		Action::FileQuickSaveState => {
			let last_save_state = model.state.borrow().last_save_state().unwrap();
			model.issue_command(MameCommand::state_save(last_save_state));
		}
		Action::FileLoadState => {
			let model_clone = model.clone();
			let fut = async move {
				let last_save_state = model_clone.state.borrow().last_save_state();
				let (initial_dir, initial_file) =
					initial_dir_and_file_from_path(last_save_state.as_deref().map(Path::new));

				let parent = model_clone.app_window().window().window_handle();
				let title = "Load State";
				let file_types = SAVE_STATE_FILE_TYPES;
				if let Some(filename) = load_file_dialog(parent, title, file_types, initial_dir, initial_file).await {
					model_clone.issue_command(MameCommand::state_load(&filename));
					model_clone.update_state(|state| Some(state.set_last_save_state(Some(filename.into()))));
				}
			};
			spawn_local(fut).unwrap();
		}
		Action::FileSaveState => {
			let model_clone = model.clone();
			let fut = async move {
				let last_save_state = model_clone.state.borrow().last_save_state();
				let (initial_dir, initial_file) =
					initial_dir_and_file_from_path(last_save_state.as_deref().map(Path::new));
				let (initial_dir, initial_file) = if initial_dir.is_some() && initial_file.is_some() {
					let initial_file = initial_file.map(Cow::Borrowed);
					(initial_dir, initial_file)
				} else {
					let initial_file = model_clone
						.suggested_initial_save_filename(SAVE_STATE_EXTENSION)
						.map(Cow::Owned);
					(None, initial_file)
				};
				let initial_file = initial_file.as_deref();

				let parent = model_clone.app_window().window().window_handle();
				let title = "Save State";
				let file_types = SAVE_STATE_FILE_TYPES;
				if let Some(filename) = save_file_dialog(parent, title, file_types, initial_dir, initial_file).await {
					model_clone.issue_command(MameCommand::state_save(&filename));
					model_clone.update_state(|state| Some(state.set_last_save_state(Some(filename.into()))));
				}
			};
			spawn_local(fut).unwrap();
		}
		Action::FileSaveScreenshot => {
			let model_clone = model.clone();
			let fut = async move {
				let model = model_clone.as_ref();
				let parent = model_clone.app_window().window().window_handle();
				let title = "Save Screenshot";
				let file_types = [(None, "png")];
				let initial_file = model.suggested_initial_save_filename("png");
				if let Some(filename) =
					save_file_dialog(parent, title, &file_types, None, initial_file.as_deref()).await
				{
					model.issue_command(MameCommand::save_snapshot(0, &filename));
				}
			};
			spawn_local(fut).unwrap();
		}
		Action::FileRecordMovie => {
			let is_recording = model
				.state
				.borrow()
				.status()
				.as_ref()
				.unwrap()
				.running
				.as_ref()
				.unwrap()
				.is_recording;

			if is_recording {
				model.issue_command(MameCommand::end_recording());
			} else {
				let model_clone = model.clone();
				let fut = async move {
					let model = model_clone.as_ref();
					let parent = model.app_window().window().window_handle();
					let title = "Record Movie";
					let file_types = MovieFormat::iter().map(|x| x.to_string()).collect::<Vec<_>>();
					let file_types = file_types.iter().map(|ext| (None, ext.as_str())).collect::<Vec<_>>();
					let initial_file = model.suggested_initial_save_filename(&MovieFormat::default().to_string());
					if let Some(filename) =
						save_file_dialog(parent, title, &file_types, None, initial_file.as_deref()).await
					{
						let movie_format = MovieFormat::try_from(Path::new(&filename)).unwrap_or_default();
						model.issue_command(MameCommand::begin_recording(&filename, movie_format));
					}
				};
				spawn_local(fut).unwrap();
			}
		}
		Action::FileDebugger => {
			model.issue_command(MameCommand::debugger());
		}
		Action::FileResetSoft => {
			model.issue_command(MameCommand::soft_reset());
		}
		Action::FileResetHard => {
			model.issue_command(MameCommand::hard_reset());
		}
		Action::FileExit => {
			model.update_state(AppState::shutdown);
		}
		Action::OptionsThrottleRate(throttle) => {
			model.issue_command(MameCommand::throttle_rate(throttle));
		}
		Action::OptionsThrottleSpeedIncrease => {
			let state = model.state.borrow();
			let current_rate = state.status().and_then(|s| s.running.as_ref()).map(|r| r.throttle_rate);
			let new_rate = THROTTLE_RATES
				.iter()
				.rev()
				.find(|&r| current_rate.is_some_and(|cr| *r > cr));
			if let Some(&new_rate) = new_rate {
				model.issue_command(MameCommand::throttle_rate(new_rate));
			}
		}
		Action::OptionsThrottleSpeedDecrease => {
			let state = model.state.borrow();
			let current_rate = state.status().and_then(|s| s.running.as_ref()).map(|r| r.throttle_rate);
			let new_rate = THROTTLE_RATES.iter().find(|&r| current_rate.is_some_and(|cr| *r < cr));
			if let Some(&new_rate) = new_rate {
				model.issue_command(MameCommand::throttle_rate(new_rate));
			}
		}
		Action::OptionsToggleWarp => {
			let is_throttled = model
				.state
				.borrow()
				.status()
				.and_then(|s| s.running.as_ref())
				.map(|r| r.is_throttled)
				.unwrap_or_default();
			model.issue_command(MameCommand::throttled(!is_throttled));
		}
		Action::OptionsFrameskip(rate) => {
			model.issue_command(MameCommand::frameskip(rate));
		}
		Action::OptionsToggleFullScreen => {
			let app_window = model.app_window();
			let window = app_window.window();
			let new_fullscreen = !window.is_fullscreen();

			window.set_fullscreen(new_fullscreen);
			let mut prefs = model.preferences.borrow_mut();
			prefs.is_fullscreen = new_fullscreen;
			prefs.fullscreen_display = window.fullscreen_display().map(|x| x.into());
		}
		Action::OptionsToggleMenuBar => {
			let has_input_using_mouse = model
				.state
				.borrow()
				.status()
				.and_then(|s| s.running.as_ref())
				.map(|r| r.has_input_using_mouse);

			if let Some(has_input_using_mouse) = has_input_using_mouse {
				let app_window = model.app_window();
				let new_visible = !app_window.get_menubar_visible();
				app_window.set_menubar_visible(new_visible);

				if has_input_using_mouse {
					let command = MameCommand::set_mouse_enabled(!new_visible).into();
					handle_action(model, command);
				}
			}
		}
		Action::OptionsToggleSound => {
			match model
				.state
				.borrow()
				.status()
				.and_then(|s| s.running.as_ref())
				.map(|r| (r.system_mute, r.sound_attenuation))
			{
				Some((Some(system_mute), _)) => {
					model.issue_command(MameCommand::set_system_mute(!system_mute));
				}
				Some((None, Some(sound_attenuation))) => {
					let is_sound_enabled = sound_attenuation > SOUND_ATTENUATION_OFF;
					let new_attenuation = if is_sound_enabled {
						SOUND_ATTENUATION_OFF
					} else {
						SOUND_ATTENUATION_ON
					};
					model.issue_command(MameCommand::set_attenuation(new_attenuation));
				}
				_ => {}
			}
		}
		Action::OptionsCheats => {
			let status_update_channel = model.status_changed_channel.clone();
			let model_clone = model.clone();
			let invoke_command = move |command| handle_action(&model_clone, command);
			let cheats = model
				.state
				.borrow()
				.status()
				.unwrap()
				.running
				.as_ref()
				.unwrap()
				.cheats
				.clone();
			let fut = dialog_cheats(model.modal_stack.clone(), cheats, status_update_channel, invoke_command);
			spawn_local(fut).unwrap();
		}
		Action::OptionsClassic => {
			model.issue_command(MameCommand::classic_menu());
		}
		Action::OptionsConsole => {
			let _ = model.state.borrow().show_console();
		}
		Action::SettingsInput(class) => {
			let status_update_channel = model.status_changed_channel.clone();
			let model_clone = model.clone();
			let invoke_command = move |command| handle_action(&model_clone, command);
			let (inputs, input_device_classes, machine_index) = {
				let state = model.state.borrow();
				let running = state.status().unwrap().running.as_ref().unwrap();
				let inputs = running.inputs.clone();
				let input_device_classes = running.input_device_classes.clone();
				let machine_index = state
					.info_db()
					.and_then(|info_db| info_db.machines().find_index(&running.machine_name).ok());
				(inputs, input_device_classes, machine_index)
			};
			let modal_stack = model.modal_stack.clone();
			match class {
				InputClass::Controller | InputClass::Keyboard | InputClass::Misc => {
					let fut = dialog_input(
						modal_stack,
						inputs,
						input_device_classes,
						class,
						status_update_channel,
						invoke_command,
					);
					spawn_local(fut).unwrap();
				}
				InputClass::Config | InputClass::DipSwitch => {
					let info_db = model.state.borrow().info_db().unwrap().clone();
					let fut = dialog_switches(
						modal_stack,
						inputs,
						info_db,
						class,
						machine_index,
						status_update_channel,
						invoke_command,
					);
					spawn_local(fut).unwrap();
				}
			};
		}
		Action::SettingsPaths(path_type) => {
			let fut = show_paths_dialog(model.clone(), path_type);
			spawn_local(fut).unwrap();
		}
		Action::SettingsToggleBuiltinCollection(col) => {
			model.modify_prefs(|prefs| {
				toggle_builtin_collection(&mut prefs.collections, col);
			});
		}
		Action::SettingsReset => model.modify_prefs(|prefs| {
			let prefs_path = {
				let state = model.state.borrow();
				let _ = prefs.save_backup(state.prefs_path());
				state.prefs_path().to_str().map(|x| x.into())
			};
			*prefs = Preferences::fresh(prefs_path);
		}),
		Action::SettingsImportMameIni => {
			let model_clone = model.clone();
			let paths = model.preferences.borrow().paths.clone();
			let fut = async move {
				match dialog_import_mame_ini(model_clone.modal_stack.clone(), paths).await {
					Ok(None) => {}
					Ok(Some(import)) => model_clone.modify_prefs(|prefs| import.apply(prefs)),
					Err(e) => {
						dialog_message_box::<OkOnly>(model_clone.modal_stack.clone(), "Error", e.to_string()).await;
					}
				}
			};
			spawn_local(fut).unwrap();
		}
		Action::HelpRefreshInfoDb => {
			model.update_state(|state| state.infodb_rebuild());
		}
		Action::HelpWebSite => {
			let _ = open::that("https://www.bletchmame.org");
		}
		Action::HelpAbout => {
			let modal = model.modal_stack.modal(|| AboutDialog::new().unwrap());
			modal.launch();
		}
		Action::MameSessionEnded => {
			model.update_state(|state| Some(state.session_ended()));
		}
		Action::MameStatusUpdate(update) => {
			model.update_state(|state| state.status_update(update));

			// special check to restore the menu bar if we're not in the emulation
			if model.state.borrow().status().is_none_or(|s| s.running.is_none()) {
				model.app_window().set_menubar_visible(true);
			}
		}
		Action::ErrorMessageBox(message) => {
			let model_clone = model.clone();
			let fut = dialog_message_box::<OkOnly>(model_clone.modal_stack.clone(), "Error", message);
			spawn_local(fut).unwrap();
		}
		Action::Start(start_args) => match start_args.preflight() {
			Ok(_) => {
				let command = MameCommand::start(&start_args).into();
				handle_action(model, command);
			}
			Err(errors) => {
				let message = errors.into_iter().map(|e| e.to_string()).collect::<String>();
				let message =
					format!("The emulation could not be started due to the following problems:\n\n{message}",);
				let fut = dialog_message_box::<OkOnly>(model.modal_stack.clone(), "Error", message);
				spawn_local(fut).unwrap();
			}
		},
		Action::IssueMameCommand(command) => {
			model.issue_command(command);
		}
		Action::Browse(collection) => {
			let collection = Rc::new(collection);
			model.modify_prefs(|prefs| {
				prefs.history_push(collection);
			});
		}
		Action::HistoryAdvance(delta) => {
			model.modify_prefs(|prefs| prefs.history_advance(delta));
		}
		Action::SearchText(search) => {
			model.modify_prefs(|prefs| {
				// modify the search text
				let current_entry = prefs.current_history_entry_mut();
				current_entry.sort_suppressed = !search.is_empty();
				current_entry.search = search;
			});
		}
		Action::ItemsSort(column_index, order) => {
			model.modify_prefs(|prefs| {
				for (index, column) in prefs.items_columns.iter_mut().enumerate() {
					column.sort = (index == column_index).then_some(order);
				}
				prefs.current_history_entry_mut().sort_suppressed = false;
			});
		}
		Action::ItemsSelectedChanged => {
			let selection = model.with_items_table_model(|x| x.current_selection());
			model.modify_prefs(|prefs| {
				prefs.current_history_entry_mut().selection = selection;
			});
		}
		Action::AddToExistingFolder(folder_index, new_items) => {
			model.modify_prefs(|prefs| {
				add_items_to_existing_folder_collection(&mut prefs.collections, folder_index, new_items);
			});
		}
		Action::AddToNewFolder(name, items) => {
			model.modify_prefs(|prefs| {
				add_items_to_new_folder_collection(&mut prefs.collections, name, items);
			});
		}
		Action::AddToNewFolderDialog(items) => {
			let existing_names = get_folder_collection_names(&model.preferences.borrow().collections);
			let model_clone = model.clone();
			let fut = async move {
				if let Some(name) = dialog_new_collection(model_clone.modal_stack.clone(), existing_names).await {
					let command = Action::AddToNewFolder(name, items);
					handle_action(&model_clone, command);
				}
			};
			spawn_local(fut).unwrap();
		}
		Action::RemoveFromFolder(name, items) => {
			model.modify_prefs(|prefs| {
				remove_items_from_folder_collection(&mut prefs.collections, name, &items);
			});
		}
		Action::MoveCollection { old_index, new_index } => {
			model.modify_prefs(|prefs| {
				// detach the collection we're moving
				let collection = prefs.collections.remove(old_index);

				if let Some(new_index) = new_index {
					// and readd it
					prefs.collections.insert(new_index, collection);
				} else {
					// the collection is being removed; we need to remove any entries that
					// might be referenced
					prefs.purge_stray_entries();
				}
			});
		}
		Action::DeleteCollectionDialog { index } => {
			let model_clone = model.clone();
			let old_name = get_collection_name(&model.preferences.borrow().collections, index).to_string();
			let fut = async move {
				let message = format!("Are you sure you want to delete \"{old_name}\"");
				if dialog_message_box::<OkCancel>(model_clone.modal_stack.clone(), "Delete", message).await
					== OkCancel::Ok
				{
					let command = Action::MoveCollection {
						old_index: index,
						new_index: None,
					};
					handle_action(&model_clone, command);
				}
			};
			spawn_local(fut).unwrap();
		}
		Action::RenameCollectionDialog { index } => {
			let existing_names = get_folder_collection_names(&model.preferences.borrow().collections);
			let model_clone = model.clone();
			let old_name = get_collection_name(&model.preferences.borrow().collections, index).to_string();
			let fut = async move {
				if let Some(new_name) =
					dialog_rename_collection(model_clone.modal_stack.clone(), existing_names, old_name).await
				{
					let command = Action::RenameCollection { index, new_name };
					handle_action(&model_clone, command);
				}
			};
			spawn_local(fut).unwrap();
		}
		Action::RenameCollection { index, new_name } => model.modify_prefs(|prefs| {
			prefs.rename_folder(index, new_name);
		}),
		Action::BookmarkCurrentCollection => {
			let (collection, _) = model.preferences.borrow().current_collection();
			model.modify_prefs(|prefs| {
				prefs.collections.push(collection);
			})
		}
		Action::LoadImageDialog { tag } => {
			let formats = {
				let state = model.state.borrow();
				let image = state
					.status()
					.and_then(|s| s.running.as_ref())
					.unwrap()
					.images
					.iter()
					.find(|x| x.tag == tag)
					.unwrap();
				image
					.details
					.formats
					.iter()
					.map(|f| Format {
						description: f.description.clone(),
						extensions: f.extensions.clone(),
					})
					.collect::<Vec<_>>()
			};

			let model_clone = model.clone();
			let fut = async move {
				if let Some(image_desc) = dialog_load_image(model_clone.modal_stack.clone(), &formats).await {
					let command = MameCommand::load_image(tag, &image_desc).into();
					handle_action(&model_clone, command);
				}
			};
			spawn_local(fut).unwrap();
		}
		Action::UnloadImage { tag } => {
			model.issue_command(MameCommand::unload_image(&tag));
		}
		Action::ConnectToSocketDialog { tag } => {
			let model_clone = model.clone();
			let fut = async move {
				if let Some(image_desc) = dialog_connect_to_socket(model_clone.modal_stack.clone()).await {
					let command = MameCommand::load_image(tag, &image_desc).into();
					handle_action(&model_clone, command);
				}
			};
			spawn_local(fut).unwrap();
		}
		Action::InfoDbBuildProgress { machine_description } => {
			model.update_state(|state| Some(state.infodb_build_progress(machine_description)))
		}
		Action::InfoDbBuildComplete => model.update_state(|state| Some(state.infodb_build_complete())),
		Action::InfoDbBuildCancel => model.update_state(|state| Some(state.infodb_build_cancel())),
		Action::ReactivateMame => model.update_state(AppState::activate),
		Action::Configure { folder_name, index } => {
			let model_clone = model.clone();
			let info_db = model.state.borrow().info_db().unwrap().clone();
			let (folder_index, item) = model
				.preferences
				.borrow()
				.collections
				.iter()
				.enumerate()
				.filter_map(|(folder_index, collection)| {
					if let PrefsCollection::Folder { name, items } = collection.as_ref() {
						(name == &folder_name).then_some((folder_index, items[index].clone()))
					} else {
						None
					}
				})
				.next()
				.unwrap();

			let fut = async move {
				let paths = model_clone.preferences.borrow().paths.clone();
				if let Some(item) = dialog_configure(model_clone.modal_stack.clone(), info_db, item, &paths).await {
					model_clone.modify_prefs(|prefs| {
						let old_collection = prefs.collections[folder_index].clone();
						let PrefsCollection::Folder { name, mut items } = old_collection.as_ref().clone() else {
							unreachable!()
						};
						items[index] = item;
						let new_collection = PrefsCollection::Folder { name, items };
						let new_collection = Rc::new(new_collection);
						prefs.collections[folder_index] = new_collection;
					})
				}
			};
			spawn_local(fut).unwrap();
		}
		Action::SeqPollDialog {
			port_tag,
			mask,
			seq_type,
			poll_type,
		} => {
			let modal_stack: ModalStack = model.modal_stack.clone();
			let (inputs, input_device_classes) = model
				.state
				.borrow()
				.status()
				.and_then(|x| x.running.as_ref())
				.map(|x| (x.inputs.clone(), x.input_device_classes.clone()))
				.unwrap_or_default();
			let status_changed_channel = model.status_changed_channel.clone();
			let model_clone = model.clone();
			let invoke_command = move |command| handle_action(&model_clone, command);
			let fut = dialog_seq_poll(
				modal_stack,
				port_tag,
				mask,
				seq_type,
				poll_type,
				inputs,
				input_device_classes,
				status_changed_channel,
				invoke_command,
			);
			spawn_local(fut).unwrap();
		}
		Action::InputXyDialog { x_input, y_input } => {
			let modal_stack: ModalStack = model.modal_stack.clone();
			let (inputs, input_device_classes) = model
				.state
				.borrow()
				.status()
				.and_then(|x| x.running.as_ref())
				.map(|running| (running.inputs.clone(), running.input_device_classes.clone()))
				.unwrap_or_default();
			let status_changed_channel = model.status_changed_channel.clone();
			let model_clone = model.clone();
			let invoke_command = move |command| handle_action(&model_clone, command);
			let fut = dialog_input_xy(
				modal_stack,
				x_input,
				y_input,
				inputs,
				input_device_classes,
				status_changed_channel,
				invoke_command,
			);
			spawn_local(fut).unwrap();
		}
		Action::InputSelectMultipleDialog { selections } => {
			let modal_stack = model.modal_stack.clone();
			let model = model.clone();
			let fut = async move {
				let command = dialog_input_select_multiple(modal_stack, selections).await;
				if let Some(command) = command {
					handle_action(&model, command);
				}
			};
			spawn_local(fut).unwrap();
		}
	};

	// finish up
	debug!(duration=?start_instant.elapsed(), "handle_action");
}

async fn show_paths_dialog(model: Rc<AppModel>, path_type: Option<PathType>) {
	let paths = model.preferences.borrow().paths.clone();
	if let Some(new_paths) = dialog_paths(model.modal_stack.clone(), paths, path_type).await {
		model.modify_prefs(|prefs| prefs.paths = new_paths.into());
	}
}

fn update_menus(model: &AppModel) {
	// calculate properties
	let state = model.state.borrow();
	let build = state.status().as_ref().map(|s| &s.build);
	let running = state.status().and_then(|s| s.running.as_ref());
	let has_mame_executable = model.preferences.borrow().paths.mame_executable.is_some();
	let is_running = running.is_some();
	let is_paused = running.as_ref().map(|r| r.is_paused).unwrap_or_default();
	let is_throttled = running.as_ref().map(|r| r.is_throttled).unwrap_or_default();
	let menu_entries_throttle_current_index = running
		.and_then(|running: &crate::status::Running| THROTTLE_RATES.iter().position(|&r| r == running.throttle_rate))
		.map(|x| i32::try_from(x).unwrap())
		.unwrap_or(-1);
	let menu_entries_frameskip_current_index = running
		.and_then(|running| FRAMESKIP_RATES.iter().position(|&r| r == running.frameskip))
		.map(|x| i32::try_from(x).unwrap())
		.unwrap_or(-1);
	let is_sound_enabled = running
		.as_ref()
		.map(|r| {
			if let Some(system_mute) = r.system_mute {
				!system_mute
			} else {
				r.sound_attenuation.is_some_and(|x| x > SOUND_ATTENUATION_OFF)
			}
		})
		.unwrap_or_default();
	let can_record_movie = running.is_some() && build.is_some_and(|b| *b >= MINIMUM_MAME_RECORD_MOVIE);
	let can_refresh_info_db = has_mame_executable && !state.is_building_infodb();
	let is_fullscreen = model.app_window().window().is_fullscreen();
	let is_recording = running.as_ref().map(|r| r.is_recording).unwrap_or_default();
	let has_last_save_state = is_running && state.last_save_state().is_some();
	let input_classes = running
		.map(|x| x.inputs.as_ref())
		.unwrap_or_default()
		.iter()
		.filter_map(|x| x.class)
		.collect::<HashSet<_>>();
	let has_cheats = running.as_ref().map(|r| !r.cheats.is_empty()).unwrap_or_default();
	let has_classic_mame_menu = running.is_some() && build.is_some_and(|b| *b >= MINIMUM_MAME_CLASSIC_MENU);

	// update the menu bar
	let app_window = model.app_window();
	app_window.set_is_paused(is_paused);
	app_window.set_is_recording(is_recording);
	app_window.set_is_throttled(is_throttled);
	app_window.set_is_fullscreen(is_fullscreen);
	app_window.set_is_sound_enabled(is_sound_enabled);
	app_window.set_menu_entries_throttle_current_index(menu_entries_throttle_current_index);
	app_window.set_menu_entries_frameskip_current_index(menu_entries_frameskip_current_index);
	app_window.set_has_last_save_state(has_last_save_state);
	app_window.set_has_cheats(has_cheats);
	app_window.set_has_classic_mame_menu(has_classic_mame_menu);
	app_window.set_can_record_movie(can_record_movie);
	app_window.set_can_refresh_info_db(can_refresh_info_db);
	app_window.set_has_input_class_controller(input_classes.contains(&InputClass::Controller));
	app_window.set_has_input_class_keyboard(input_classes.contains(&InputClass::Keyboard));
	app_window.set_has_input_class_misc(input_classes.contains(&InputClass::Misc));
	app_window.set_has_input_class_config(input_classes.contains(&InputClass::Config));
	app_window.set_has_input_class_dipswitch(input_classes.contains(&InputClass::DipSwitch));
}

/// updates all UI elements (except items and items columns models) to reflect the current history item
fn update_ui_for_current_history_item(model: &AppModel) {
	// tracing
	let span = debug_span!("update_ui_for_current_history_item");
	let _guard = span.enter();
	let start_instant = Instant::now();

	// the basics
	let app_window = model.app_window();
	let prefs = model.preferences.borrow();
	let search = prefs.current_history_entry().search.clone();

	// update back/forward buttons
	app_window.set_history_can_go_back(prefs.can_history_advance(-1));
	app_window.set_history_can_go_forward(prefs.can_history_advance(1));

	// update search text bar
	app_window.set_items_search_text(SharedString::from(&search));

	// identify the currently selected collection
	let (collection, collection_index) = prefs.current_collection();
	let collection_index = collection_index.and_then(|x| i32::try_from(x).ok()).unwrap_or(-1);

	// update current collection text
	let current_collection_desc = model
		.state
		.borrow()
		.info_db()
		.map(|info_db| collection.description(info_db))
		.unwrap_or_default();
	app_window.set_current_collection_text(current_collection_desc.as_ref().into());

	// update the bookmark this collection icon
	let is_collection_in_list = prefs.collections.contains(&collection);
	app_window.set_bookmark_collection_enabled(!is_collection_in_list);

	// update the collections view
	let app_window_weak = app_window.as_weak();
	model.with_collections_view_model(|x| {
		x.callback_after_refresh(async move {
			app_window_weak
				.unwrap()
				.invoke_collections_view_select(collection_index);
		})
	});

	// update the snap image
	model
		.snap_image
		.set_current_item(prefs.current_history_entry().selection.first());

	// and finish tracing
	debug!(duration=?start_instant.elapsed(), "update_ui_for_current_history_item() completed");
}

fn update_items_columns_model(model: &AppModel) {
	// tracing
	let span = debug_span!("update_items_columns_model");
	let _guard = span.enter();
	let start_instant = Instant::now();

	let app_window = model.app_window();
	let prefs = model.preferences.borrow();

	let items_columns = app_window.get_items_columns();
	let sort_suppressed = prefs.current_history_entry().sort_suppressed;
	for (index, column) in prefs.items_columns.iter().enumerate() {
		if let Some(mut data) = items_columns.row_data(index) {
			let sort_order = (!sort_suppressed).then_some(column.sort).flatten();
			data.sort_order = match sort_order {
				None => i_slint_core::items::SortOrder::Unsorted,
				Some(SortOrder::Ascending) => i_slint_core::items::SortOrder::Ascending,
				Some(SortOrder::Descending) => i_slint_core::items::SortOrder::Descending,
			};
			items_columns.set_row_data(index, data);
		}
	}

	// and finish tracing
	debug!(duration=?start_instant.elapsed(), "update_items_columns_model() completed");
}

fn update_items_model_for_current_prefs(model: &AppModel) {
	// tracing
	let span = debug_span!("update_items_model_for_current_prefs");
	let _guard = span.enter();
	let start_instant = Instant::now();

	model.with_items_table_model(move |items_model| {
		let prefs = model.preferences.borrow();
		let entry = prefs.current_history_entry();
		let (current_collection, _) = prefs.current_collection();
		let column_types = prefs.items_columns.iter().map(|col| col.column_type).collect();
		let sorting = (!entry.sort_suppressed)
			.then(|| {
				prefs
					.items_columns
					.iter()
					.filter_map(|col| col.sort.map(|x| (col.column_type, x)))
					.next()
			})
			.flatten();

		items_model.update(
			None,
			Some(&prefs.paths.software_lists),
			Some(current_collection),
			Some(column_types),
			Some(entry.search.as_str()),
			Some(sorting),
			Some(&entry.selection),
		);
	});

	// and finish tracing
	debug!(duration=?start_instant.elapsed(), "update_items_model_for_current_prefs() completed");
}

fn update_builtin_collections_menu_checked(model: &AppModel, prefs: &Preferences) {
	let builtin_collections_checked = BuiltinCollection::iter()
		.map(|b| {
			prefs
				.collections
				.iter()
				.any(|c| matches!(c.as_ref(), PrefsCollection::Builtin(x) if *x == b))
		})
		.collect::<Vec<_>>();
	let builtin_collections_checked = VecModel::from(builtin_collections_checked);
	let builtin_collections_checked = ModelRc::new(builtin_collections_checked);
	model
		.app_window()
		.set_menu_entries_builtin_collections_checked(builtin_collections_checked);
}

fn update_prefs(model: &Rc<AppModel>) {
	model.modify_prefs(|prefs| {
		// update window size
		let physical_size = model.app_window().window().size();
		let logical_size = physical_size.to_logical(model.app_window().window().scale_factor());
		prefs.window_size = Some(logical_size.into());

		let items_columns = model.app_window().get_items_columns();
		for (index, column) in prefs.items_columns.iter_mut().enumerate() {
			if let Some(data) = items_columns.row_data(index) {
				column.width = data.width;
			}
		}

		// update collections related prefs
		prefs.collections = model.with_collections_view_model(|x| x.get_all());
	});
}

fn update_empty_reason(model: &AppModel, empty_reason: Option<EmptyReason>) {
	let app_window = model.app_window();
	let reason_string = empty_reason.map(|x| format!("{x}")).unwrap_or_default().into();
	app_window.set_is_empty_reason(reason_string);
}

fn items_set_sorting(model: &Rc<AppModel>, column: i32, order: SortOrder) {
	let column = usize::try_from(column).unwrap();
	let command = Action::ItemsSort(column, order);
	handle_action(model, command);
}
