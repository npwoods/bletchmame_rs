use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;
use std::time::Instant;

use muda::accelerator::Accelerator;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::LogicalSize;
use slint::Model;
use slint::ModelRc;
use slint::SharedString;
use slint::TableColumn;
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

use crate::appcommand::AppCommand;
use crate::appstate::AppState;
use crate::backend::BackendRuntime;
use crate::backend::ChildWindow;
use crate::backend::WindowExt as WindowExt_1;
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
use crate::guiutils::is_context_menu_event;
use crate::guiutils::menuing::accel;
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
use crate::status::InputClass;
use crate::status::Status;
use crate::ui::AboutDialog;
use crate::ui::AppWindow;
use crate::ui::ReportIssue;
use crate::version::MameVersion;

const SOUND_ATTENUATION_OFF: i32 = -32;
const SOUND_ATTENUATION_ON: i32 = 0;

const SAVE_STATE_EXTENSION: &str = "sta";
const SAVE_STATE_FILE_TYPES: &[(Option<&str>, &str)] = &[(Some("MAME Saved State Files"), SAVE_STATE_EXTENSION)];

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

		// reborrow prefs (but not mutably)
		let prefs = self.preferences.borrow();

		// save (ignore errors)
		let _ = self.preferences.borrow().save(self.state.borrow().prefs_path());

		// react to all of the possible changes
		self.update_state(|state| state.update_paths(&prefs.paths));
		if prefs.collections != old_prefs.collections {
			info!("modify_prefs(): prefs.collection changed");
			let info_db = self.state.borrow().info_db().cloned();
			self.with_collections_view_model(|x| x.update(info_db, &prefs.collections));
		}

		let must_update_for_current_history_item = prefs.current_history_entry() != old_prefs.current_history_entry();
		let must_update_items_columns_model =
			must_update_for_current_history_item || prefs.items_columns != old_prefs.items_columns;

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
	// prepare the menu bar; we explicitly want to do this before `wait_for_window_ready()`
	app_window.set_menu_items_builtin_collections(ModelRc::new(VecModel::from(
		BuiltinCollection::iter()
			.map(|x| SharedString::from(x.to_string()))
			.collect::<Vec<_>>(),
	)));

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
		.unwrap_or_else(|| Preferences::fresh(prefs_path.to_str().map(|x| x.to_string())));

	// update window preferences
	if let Some(window_size) = &preferences.window_size {
		let physical_size = LogicalSize::from(*window_size).to_physical(app_window.window().scale_factor());
		app_window.window().set_size(physical_size);
	}

	// create the window stack
	let modal_stack = ModalStack::new(args.backend_runtime.clone(), app_window);

	// create the model
	let model = AppModel {
		app_window_weak: app_window.as_weak(),
		backend_runtime: args.backend_runtime,
		modal_stack,
		preferences: RefCell::new(preferences),
		state: RefCell::new(AppState::bogus()),
		status_changed_channel: Channel::default(),
		child_window: RefCell::new(None),
	};
	let model = Rc::new(model);

	// set full screen
	{
		let prefs = model.preferences.borrow();
		app_window
			.window()
			.set_fullscreen_with_display(prefs.is_fullscreen, prefs.fullscreen_display.as_deref());
	}

	// attach the menu bar (either natively or with an approximation using Slint); looking forward to Slint having first class menuing support
	let model_clone = model.clone();
	app_window.on_menu_item_activated(move |parent_title, title| {
		// dispatch the command
		if let Some(command) = menu_item_command(Some(&parent_title), &title) {
			handle_command(&model_clone, command);
		}
	});
	let model_clone = model.clone();
	app_window.on_menu_item_command(move |command_string| {
		if let Some(command) = AppCommand::decode_from_slint(command_string) {
			handle_command(&model_clone, command);
		}
	});
	let model_clone = model.clone();
	app_window.on_minimum_mame(move |major, minor| {
		let major = major.try_into().unwrap();
		let minor = minor.try_into().unwrap();
		let version = MameVersion::new(major, minor);
		model_clone
			.state
			.borrow()
			.status()
			.map(|status| status.running.is_some() && status.build >= version)
			.unwrap_or(false)
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
		("Pause", Some(AppCommand::FilePause)),
		("F7", Some(AppCommand::FileQuickLoadState)),
		("Shift+F7", Some(AppCommand::FileQuickLoadState)),
		("Ctrl+F7", Some(AppCommand::FileLoadState)),
		("Ctrl+Shift+F7", Some(AppCommand::FileLoadState)),
		("F12", Some(AppCommand::FileSaveScreenshot)),
		("Shift+F12", Some(AppCommand::FileRecordMovie)),
		("Ctrl+Alt+X", Some(AppCommand::FileExit)),
		("F9", None),
		("F8", None),
		("F10", Some(AppCommand::OptionsToggleWarp)),
		("F11", Some(AppCommand::OptionsToggleFullScreen)),
		("ScrLk", Some(AppCommand::OptionsToggleMenuBar)),
	];
	let accelerator_command_map =
		HashMap::<Accelerator, AppCommand>::from_iter(accelerator_command_map.into_iter().filter_map(
			|(accelerator, command)| {
				accel(accelerator).and_then(|accelerator| command.map(|command| (accelerator, command)))
			},
		));
	let model_clone = model.clone();
	model
		.backend_runtime
		.install_muda_accelerator_handler(app_window.window(), move |accelerator| {
			let command = accelerator_command_map.get(accelerator);
			if let Some(command) = command {
				handle_command(&model_clone, command.clone());
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
			let command = AppCommand::Browse(collection);
			handle_command(&model_clone, command);
		}
	});

	// set up back/foward buttons
	let model_clone = model.clone();
	app_window.on_history_advance_clicked(move |delta| {
		let delta = delta.try_into().unwrap();
		handle_command(&model_clone, AppCommand::HistoryAdvance(delta));
	});

	// set up bookmark collection button
	let model_clone = model.clone();
	app_window.on_bookmark_collection_clicked(move || {
		handle_command(&model_clone, AppCommand::BookmarkCurrentCollection);
	});

	// set up items columns
	let items_columns = model
		.preferences
		.borrow()
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
		let command = AppCommand::SearchText(search.into());
		handle_command(&model_clone, command);
	});
	app_window.set_items_search_text(SharedString::from(
		&model.preferences.borrow().current_history_entry().search,
	));
	let model_clone = model.clone();
	app_window.on_items_current_row_changed(move || {
		let command = AppCommand::ItemsSelectedChanged;
		handle_command(&model_clone, command);
	});

	// for when we shut down
	let model_clone = model.clone();
	app_window.window().on_close_requested(move || {
		let command = AppCommand::FileExit;
		handle_command(&model_clone, command);
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
		handle_command(&model_clone, command);
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
		handle_command(&model_clone, command);
	});

	// initial updates
	update_ui_for_current_history_item(&model);
	update_items_columns_model(&model);
	update_items_model_for_current_prefs(&model);

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
		handle_command(&model, command);
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

fn menu_item_command(parent_title: Option<&str>, title: &str) -> Option<AppCommand> {
	let command = match (parent_title, title) {
		// File menu
		(_, "Stop") => Some(AppCommand::FileStop),
		(_, "Pause") => Some(AppCommand::FilePause),
		(_, "Devices and Images...") => Some(AppCommand::FileDevicesAndImages),
		(_, "Quick Load State") => Some(AppCommand::FileQuickLoadState),
		(_, "Quick Save State") => Some(AppCommand::FileQuickLoadState),
		(_, "Load State...") => Some(AppCommand::FileLoadState),
		(_, "Save State...") => Some(AppCommand::FileLoadState),
		(_, "Save Screenshot...") => Some(AppCommand::FileSaveScreenshot),
		(_, "Record Movie...") => Some(AppCommand::FileRecordMovie),
		(_, "Stop Recording") => Some(AppCommand::FileRecordMovie),
		(_, "Debugger...") => Some(AppCommand::FileDebugger),
		(_, "Soft Reset") => Some(AppCommand::FileResetSoft),
		(_, "Hard Reset") => Some(AppCommand::FileResetHard),
		(_, "Exit") => Some(AppCommand::FileExit),

		// Options menu
		(Some("Throttle"), "Increase Speed") => None,
		(Some("Throttle"), "Decrease Speed") => None,
		(Some("Throttle"), "Warp mode") => Some(AppCommand::OptionsToggleWarp),
		(Some("Throttle"), rate) => {
			let rate = rate.strip_suffix('%').unwrap().parse().unwrap();
			Some(AppCommand::OptionsThrottleRate(rate))
		}
		(_, "Full Screen") => Some(AppCommand::OptionsToggleFullScreen),
		(_, "Toggle Menu Bar") => Some(AppCommand::OptionsToggleMenuBar),
		(_, "Sound") => Some(AppCommand::OptionsToggleSound),
		(_, "Cheats...") => Some(AppCommand::OptionsCheats),
		(_, "Classic MAME Menu") => Some(AppCommand::OptionsClassic),
		(_, "Console") => Some(AppCommand::OptionsConsole),

		// Settings menu
		(_, "Joysticks and Controllers...") => Some(AppCommand::SettingsInput(InputClass::Controller)),
		(_, "Keyboard...") => Some(AppCommand::SettingsInput(InputClass::Keyboard)),
		(_, "Miscellaneous Input...") => Some(AppCommand::SettingsInput(InputClass::Misc)),
		(_, "Configuration...") => Some(AppCommand::SettingsInput(InputClass::Config)),
		(_, "DIP Switches...") => Some(AppCommand::SettingsInput(InputClass::DipSwitch)),
		(_, "Paths...") => Some(AppCommand::SettingsPaths(None)),
		(Some("Builtin Collections"), col) => {
			let col = BuiltinCollection::from_str(col).unwrap();
			Some(AppCommand::SettingsToggleBuiltinCollection(col))
		}
		(_, "Reset Settings To Default") => Some(AppCommand::SettingsReset),
		(_, "Import MAME INI...") => Some(AppCommand::SettingsImportMameIni),

		// Help menu
		(_, "Refresh MAME machine info...") => Some(AppCommand::HelpRefreshInfoDb),
		(_, "BletchMAME web site...") => Some(AppCommand::HelpWebSite),
		(_, "About...") => Some(AppCommand::HelpAbout),

		// Anything else
		(_, _) => None,
	};
	debug!(parent_title=?parent_title, title=?title, command=?command, "menu_item_command");
	command
}

fn handle_command(model: &Rc<AppModel>, command: AppCommand) {
	// tracing
	let command_str: &'static str = (&command).into();
	let span = if command.is_frequent() {
		debug_span!("handle_command", command = command_str)
	} else {
		info_span!("handle_command", command = command_str)
	};
	let _guard = span.enter();
	info!(command=?&command, "handle_command()");
	let start_instant = Instant::now();

	match command {
		AppCommand::FileStop => {
			model.issue_command(MameCommand::stop());
		}
		AppCommand::FilePause => {
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
		AppCommand::FileDevicesAndImages => {
			let info_db = model.state.borrow().info_db().cloned().unwrap();
			let diconfig = DevicesImagesConfig::new(info_db);
			let diconfig = diconfig.update_status(model.state.borrow().status().as_ref().unwrap());
			let status_update_channel = model.status_changed_channel.clone();
			let model_clone = model.clone();
			let invoke_command = move |command| handle_command(&model_clone, command);
			let fut = dialog_devices_and_images(
				model.modal_stack.clone(),
				diconfig,
				status_update_channel,
				invoke_command,
			);
			spawn_local(fut).unwrap();
		}
		AppCommand::FileQuickLoadState => {
			let last_save_state = model.state.borrow().last_save_state().unwrap();
			model.issue_command(MameCommand::state_load(last_save_state));
		}
		AppCommand::FileQuickSaveState => {
			let last_save_state = model.state.borrow().last_save_state().unwrap();
			model.issue_command(MameCommand::state_save(last_save_state));
		}
		AppCommand::FileLoadState => {
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
		AppCommand::FileSaveState => {
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
		AppCommand::FileSaveScreenshot => {
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
		AppCommand::FileRecordMovie => {
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
		AppCommand::FileDebugger => {
			model.issue_command(MameCommand::debugger());
		}
		AppCommand::FileResetSoft => {
			model.issue_command(MameCommand::soft_reset());
		}
		AppCommand::FileResetHard => {
			model.issue_command(MameCommand::hard_reset());
		}
		AppCommand::FileExit => {
			model.update_state(AppState::shutdown);
		}
		AppCommand::OptionsThrottleRate(throttle) => {
			model.issue_command(MameCommand::throttle_rate(throttle));
		}
		AppCommand::OptionsToggleWarp => {
			let is_throttled = model
				.state
				.borrow()
				.status()
				.and_then(|s| s.running.as_ref())
				.map(|r| r.is_throttled)
				.unwrap_or_default();
			model.issue_command(MameCommand::throttled(!is_throttled));
		}
		AppCommand::OptionsToggleFullScreen => {
			let app_window = model.app_window();
			let window = app_window.window();
			let new_fullscreen = !window.is_fullscreen();

			window.set_fullscreen(new_fullscreen);
			let mut prefs = model.preferences.borrow_mut();
			prefs.is_fullscreen = new_fullscreen;
			prefs.fullscreen_display = window.fullscreen_display();
		}
		AppCommand::OptionsToggleMenuBar => {
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
					handle_command(model, command);
				}
			}
		}
		AppCommand::OptionsToggleSound => {
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
		AppCommand::OptionsCheats => {
			let status_update_channel = model.status_changed_channel.clone();
			let model_clone = model.clone();
			let invoke_command = move |command| handle_command(&model_clone, command);
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
		AppCommand::OptionsClassic => {
			model.issue_command(MameCommand::classic_menu());
		}
		AppCommand::OptionsConsole => {
			let _ = model.state.borrow().show_console();
		}
		AppCommand::SettingsInput(class) => {
			let status_update_channel = model.status_changed_channel.clone();
			let model_clone = model.clone();
			let invoke_command = move |command| handle_command(&model_clone, command);
			let (inputs, input_device_classes) = {
				let state = model.state.borrow();
				let running = state.status().unwrap().running.as_ref().unwrap();
				let inputs = running.inputs.clone();
				let input_device_classes = running.input_device_classes.clone();
				(inputs, input_device_classes)
			};
			let fut = dialog_input(
				model.modal_stack.clone(),
				inputs,
				input_device_classes,
				class,
				status_update_channel,
				invoke_command,
			);
			spawn_local(fut).unwrap();
		}
		AppCommand::SettingsPaths(path_type) => {
			let fut = show_paths_dialog(model.clone(), path_type);
			spawn_local(fut).unwrap();
		}
		AppCommand::SettingsToggleBuiltinCollection(col) => {
			model.modify_prefs(|prefs| {
				toggle_builtin_collection(&mut prefs.collections, col);
			});
		}
		AppCommand::SettingsReset => model.modify_prefs(|prefs| {
			let prefs_path = {
				let state = model.state.borrow();
				let _ = prefs.save_backup(state.prefs_path());
				state.prefs_path().to_str().map(str::to_string)
			};
			*prefs = Preferences::fresh(prefs_path);
		}),
		AppCommand::SettingsImportMameIni => {
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
		AppCommand::HelpRefreshInfoDb => {
			model.update_state(|state| state.infodb_rebuild());
		}
		AppCommand::HelpWebSite => {
			let _ = open::that("https://www.bletchmame.org");
		}
		AppCommand::HelpAbout => {
			let modal = model.modal_stack.modal(|| AboutDialog::new().unwrap());
			modal.launch();
		}
		AppCommand::MameSessionEnded => {
			model.update_state(|state| Some(state.session_ended()));
		}
		AppCommand::MameStatusUpdate(update) => {
			model.update_state(|state| state.status_update(update));

			// special check to restore the menu bar if we're not in the emulation
			if model.state.borrow().status().is_none_or(|s| s.running.is_none()) {
				model.app_window().set_menubar_visible(true);
			}
		}
		AppCommand::ErrorMessageBox(message) => {
			let model_clone = model.clone();
			let fut = dialog_message_box::<OkOnly>(model_clone.modal_stack.clone(), "Error", message);
			spawn_local(fut).unwrap();
		}
		AppCommand::Start(start_args) => match start_args.preflight() {
			Ok(_) => {
				let command = MameCommand::start(&start_args).into();
				handle_command(model, command);
			}
			Err(errors) => {
				let message = errors.into_iter().map(|e| e.to_string()).collect::<String>();
				let message =
					format!("The emulation could not be started due to the following problems:\n\n{message}",);
				let fut = dialog_message_box::<OkOnly>(model.modal_stack.clone(), "Error", message);
				spawn_local(fut).unwrap();
			}
		},
		AppCommand::IssueMameCommand(command) => {
			model.issue_command(command);
		}
		AppCommand::Browse(collection) => {
			let collection = Rc::new(collection);
			model.modify_prefs(|prefs| {
				prefs.history_push(collection);
			});
		}
		AppCommand::HistoryAdvance(delta) => {
			model.modify_prefs(|prefs| prefs.history_advance(delta));
		}
		AppCommand::SearchText(search) => {
			model.modify_prefs(|prefs| {
				// modify the search text
				let current_entry = prefs.current_history_entry_mut();
				current_entry.sort_suppressed = !search.is_empty();
				current_entry.search = search;
			});
		}
		AppCommand::ItemsSort(column_index, order) => {
			model.modify_prefs(|prefs| {
				for (index, column) in prefs.items_columns.iter_mut().enumerate() {
					column.sort = (index == column_index).then_some(order);
				}
				prefs.current_history_entry_mut().sort_suppressed = false;
			});
		}
		AppCommand::ItemsSelectedChanged => {
			let selection = model.with_items_table_model(|x| x.current_selection());
			model.modify_prefs(|prefs| {
				prefs.current_history_entry_mut().selection = selection;
			});
		}
		AppCommand::AddToExistingFolder(folder_index, new_items) => {
			model.modify_prefs(|prefs| {
				add_items_to_existing_folder_collection(&mut prefs.collections, folder_index, new_items);
			});
		}
		AppCommand::AddToNewFolder(name, items) => {
			model.modify_prefs(|prefs| {
				add_items_to_new_folder_collection(&mut prefs.collections, name, items);
			});
		}
		AppCommand::AddToNewFolderDialog(items) => {
			let existing_names = get_folder_collection_names(&model.preferences.borrow().collections);
			let model_clone = model.clone();
			let fut = async move {
				if let Some(name) = dialog_new_collection(model_clone.modal_stack.clone(), existing_names).await {
					let command = AppCommand::AddToNewFolder(name, items);
					handle_command(&model_clone, command);
				}
			};
			spawn_local(fut).unwrap();
		}
		AppCommand::RemoveFromFolder(name, items) => {
			model.modify_prefs(|prefs| {
				remove_items_from_folder_collection(&mut prefs.collections, name, &items);
			});
		}
		AppCommand::MoveCollection { old_index, new_index } => {
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
		AppCommand::DeleteCollectionDialog { index } => {
			let model_clone = model.clone();
			let old_name = get_collection_name(&model.preferences.borrow().collections, index).to_string();
			let fut = async move {
				let message = format!("Are you sure you want to delete \"{old_name}\"");
				if dialog_message_box::<OkCancel>(model_clone.modal_stack.clone(), "Delete", message).await
					== OkCancel::Ok
				{
					let command = AppCommand::MoveCollection {
						old_index: index,
						new_index: None,
					};
					handle_command(&model_clone, command);
				}
			};
			spawn_local(fut).unwrap();
		}
		AppCommand::RenameCollectionDialog { index } => {
			let existing_names = get_folder_collection_names(&model.preferences.borrow().collections);
			let model_clone = model.clone();
			let old_name = get_collection_name(&model.preferences.borrow().collections, index).to_string();
			let fut = async move {
				if let Some(new_name) =
					dialog_rename_collection(model_clone.modal_stack.clone(), existing_names, old_name).await
				{
					let command = AppCommand::RenameCollection { index, new_name };
					handle_command(&model_clone, command);
				}
			};
			spawn_local(fut).unwrap();
		}
		AppCommand::RenameCollection { index, new_name } => model.modify_prefs(|prefs| {
			prefs.rename_folder(index, new_name);
		}),
		AppCommand::BookmarkCurrentCollection => {
			let (collection, _) = model.preferences.borrow().current_collection();
			model.modify_prefs(|prefs| {
				prefs.collections.push(collection);
			})
		}
		AppCommand::LoadImageDialog { tag } => {
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
					handle_command(&model_clone, command);
				}
			};
			spawn_local(fut).unwrap();
		}
		AppCommand::UnloadImage { tag } => {
			model.issue_command(MameCommand::unload_image(&tag));
		}
		AppCommand::ConnectToSocketDialog { tag } => {
			let model_clone = model.clone();
			let fut = async move {
				if let Some(image_desc) = dialog_connect_to_socket(model_clone.modal_stack.clone()).await {
					let command = MameCommand::load_image(tag, &image_desc).into();
					handle_command(&model_clone, command);
				}
			};
			spawn_local(fut).unwrap();
		}
		AppCommand::InfoDbBuildProgress { machine_description } => {
			model.update_state(|state| Some(state.infodb_build_progress(machine_description)))
		}
		AppCommand::InfoDbBuildComplete => model.update_state(|state| Some(state.infodb_build_complete())),
		AppCommand::InfoDbBuildCancel => model.update_state(|state| Some(state.infodb_build_cancel())),
		AppCommand::ReactivateMame => model.update_state(AppState::activate),
		AppCommand::Configure { folder_name, index } => {
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
		AppCommand::SeqPollDialog {
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
			let invoke_command = move |command| handle_command(&model_clone, command);
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
		AppCommand::InputXyDialog { x_input, y_input } => {
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
			let invoke_command = move |command| handle_command(&model_clone, command);
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
		AppCommand::InputSelectMultipleDialog { selections } => {
			let modal_stack = model.modal_stack.clone();
			let model = model.clone();
			let fut = async move {
				let command = dialog_input_select_multiple(modal_stack, selections).await;
				if let Some(command) = command {
					handle_command(&model, command);
				}
			};
			spawn_local(fut).unwrap();
		}
	};

	// finish up
	debug!(duration=?start_instant.elapsed(), "handle_command");
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
	let running = state.status().and_then(|s| s.running.as_ref());
	let has_mame_executable = model.preferences.borrow().paths.mame_executable.is_some();
	let is_running = running.is_some();
	let is_paused = running.as_ref().map(|r| r.is_paused).unwrap_or_default();
	let is_throttled = running.as_ref().map(|r| r.is_throttled).unwrap_or_default();
	let throttle_rate = running.as_ref().map(|r| r.throttle_rate);
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

	// update the menu bar
	let app_window = model.app_window();
	app_window.set_is_paused(is_paused);
	app_window.set_is_recording(is_recording);
	app_window.set_is_throttled(is_throttled);
	app_window.set_is_fullscreen(is_fullscreen);
	app_window.set_is_sound_enabled(is_sound_enabled);
	app_window.set_current_throttle_rate(throttle_rate.map(|x| (x * 100.0) as i32).unwrap_or(-1));
	app_window.set_has_last_save_state(has_last_save_state);
	app_window.set_has_cheats(has_cheats);
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
	let command = AppCommand::ItemsSort(column, order);
	handle_command(model, command);
}
