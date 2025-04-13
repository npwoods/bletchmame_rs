use std::borrow::Cow;
use std::cell::RefCell;
use std::convert::Infallible;
use std::ops::ControlFlow;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;

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
use slint::invoke_from_event_loop;
use slint::quit_event_loop;
use slint::spawn_local;
use tracing::Level;
use tracing::event;

use crate::appcommand::AppCommand;
use crate::appstate::AppState;
use crate::channel::Channel;
use crate::childwindow::ChildWindow;
use crate::collections::add_items_to_existing_folder_collection;
use crate::collections::add_items_to_new_folder_collection;
use crate::collections::get_collection_name;
use crate::collections::get_folder_collection_names;
use crate::collections::get_folder_collections;
use crate::collections::remove_items_from_folder_collection;
use crate::collections::toggle_builtin_collection;
use crate::devimageconfig::DevicesImagesConfig;
use crate::dialogs::configure::dialog_configure;
use crate::dialogs::devimages::dialog_devices_and_images;
use crate::dialogs::file::initial_dir_and_file_from_path;
use crate::dialogs::file::load_file_dialog;
use crate::dialogs::file::save_file_dialog;
use crate::dialogs::image::Format;
use crate::dialogs::image::dialog_load_image;
use crate::dialogs::messagebox::OkCancel;
use crate::dialogs::messagebox::OkOnly;
use crate::dialogs::messagebox::dialog_message_box;
use crate::dialogs::namecollection::dialog_new_collection;
use crate::dialogs::namecollection::dialog_rename_collection;
use crate::dialogs::paths::dialog_paths;
use crate::dialogs::socket::dialog_connect_to_socket;
use crate::guiutils::is_context_menu_event;
use crate::guiutils::menuing::MenuExt;
use crate::guiutils::menuing::MenuItemKindExt;
use crate::guiutils::menuing::MenuItemUpdate;
use crate::guiutils::menuing::accel;
use crate::guiutils::modal::Modal;
use crate::history::History;
use crate::models::collectionsview::CollectionsViewModel;
use crate::models::itemstable::EmptyReason;
use crate::models::itemstable::ItemsTableModel;
use crate::platform::WindowExt;
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
use crate::status::Status;
use crate::ui::AboutDialog;
use crate::ui::AppWindow;
use crate::ui::ReportIssue;

const LOG_COMMANDS: Level = Level::INFO;
const LOG_PREFS: Level = Level::INFO;
const LOG_MENUING: Level = Level::DEBUG;

const SOUND_ATTENUATION_OFF: i32 = -32;
const SOUND_ATTENUATION_ON: i32 = 0;

const SAVE_STATE_EXTENSION: &str = "sta";
const SAVE_STATE_FILE_TYPES: &[(Option<&str>, &str)] = &[(Some("MAME Saved State Files"), SAVE_STATE_EXTENSION)];

/// Arguments to the application (derivative from the command line); almost all of this
/// are power user features or diagnostics
#[derive(Debug)]
pub struct AppArgs {
	pub prefs_path: PathBuf,
	pub mame_stderr: MameStderr,
}

struct AppModel {
	app_window_weak: Weak<AppWindow>,
	preferences: RefCell<Preferences>,
	state: RefCell<AppState>,
	status_changed_channel: Channel<Status>,
	child_window: ChildWindow,
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
			event!(LOG_PREFS, "modify_prefs(): prefs.collection changed");
			let info_db = self.state.borrow().info_db().cloned();
			self.with_collections_view_model(|x| x.update(info_db, &prefs.collections));
		}
		if prefs.current_history_entry() != old_prefs.current_history_entry()
			|| prefs.current_collection() != old_prefs.current_collection()
		{
			event!(LOG_PREFS, "modify_prefs(): current history_entry/collection] changed");
			update_ui_for_current_history_item(self);
		}
		if prefs.items_columns != old_prefs.items_columns {
			event!(LOG_PREFS, "modify_prefs(): items_columns changed");
			update_ui_for_sort_changes(self);
		}
		if prefs.paths.software_lists != old_prefs.paths.software_lists {
			event!(LOG_PREFS, "modify_prefs(): paths.software_lists changed");
			software_paths_updated(self);
		}
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
				items_model.info_db_changed(info_db);
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
			self.child_window.set_visible(running.is_some());

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

	pub fn issue_command(&self, command: MameCommand<'_>) {
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

pub fn create(args: AppArgs) -> AppWindow {
	// create the main "App" window
	let app_window = AppWindow::new().expect("Failed to create main window");

	// child window for MAME to attach to
	let child_window = ChildWindow::new(app_window.window()).expect("Failed to create child window");

	// prepare the menu bar
	app_window.set_menu_items_builtin_collections(ModelRc::new(VecModel::from(
		BuiltinCollection::all_values()
			.iter()
			.map(BuiltinCollection::to_string)
			.map(SharedString::from)
			.collect::<Vec<_>>(),
	)));
	let app_window_weak = app_window.as_weak();
	invoke_from_event_loop(move || {
		// need to invoke from event loop so this can happen after menu rebuild
		app_window_weak.unwrap().window().with_muda_menu(|menu_bar| {
			menu_bar.visit((), |_, sub_menu, item| {
				if let Some(title) = item.text() {
					let parent_title = sub_menu.map(|x| x.text());
					let (command, accelerator) = menu_item_info(parent_title.as_deref(), &title);

					if command.is_none() {
						item.set_enabled(false);
					}
					if let Some(accelerator) = accelerator {
						item.set_accelerator(Some(accelerator)).unwrap();
					}
				}
				ControlFlow::<Infallible>::Continue(())
			});
		});
	})
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
	app_window.window().set_fullscreen(preferences.is_fullscreen);

	// create the model
	let model = AppModel {
		app_window_weak: app_window.as_weak(),
		preferences: RefCell::new(preferences),
		state: RefCell::new(AppState::bogus()),
		status_changed_channel: Channel::default(),
		child_window,
	};
	let model = Rc::new(model);

	// attach the menu bar (either natively or with an approximation using Slint); looking forward to Slint having first class menuing support
	let model_clone = model.clone();
	app_window.on_menu_item_activated(move |parent_title, title| {
		// hack to work around Muda automatically changing the check mark value
		model_clone.app_window().window().with_muda_menu(|menu_bar| {
			menu_bar.visit((), |_, sub_menu, item| {
				if sub_menu.is_some_and(|x| x.text().as_str() == parent_title.as_str())
					&& item.text().is_some_and(|x| x.as_str() == title.as_str())
				{
					if let Some(item) = item.as_check_menuitem() {
						item.set_checked(!item.is_checked());
					}
					ControlFlow::Break(())
				} else {
					ControlFlow::Continue(())
				}
			});
		});

		// dispatch the command
		if let (Some(command), _) = menu_item_info(Some(&parent_title), &title) {
			handle_command(&model_clone, command);
		}
	});
	let model_clone = model.clone();
	app_window.on_menu_item_command(move |command_string| {
		if let Some(command) = AppCommand::decode_from_slint(command_string) {
			handle_command(&model_clone, command);
		}
	});

	// create a repeating future that will update the child window forever
	let model_weak = Rc::downgrade(&model);
	app_window.on_size_changed(move || {
		if let Some(model) = model_weak.upgrade() {
			// set the child window size
			model.child_window.update(model.app_window().window(), 0.0);
		}
	});

	// set up the collections view model
	let collections_view_model = CollectionsViewModel::new(app_window.as_weak());
	let collections_view_model = Rc::new(collections_view_model);
	app_window.set_collections_model(ModelRc::new(collections_view_model.clone()));

	// set up items view model
	let current_collection = {
		let prefs = model.preferences.borrow();
		let (collection, _) = prefs.current_collection();
		collection
	};
	let selection = SelectionManager::new(
		&app_window,
		AppWindow::get_items_view_selected_index,
		AppWindow::invoke_items_view_select,
	);
	let model_clone = model.clone();
	let empty_callback = move |empty_reason| {
		update_empty_reason(&model_clone, empty_reason);
	};
	let items_model = {
		let prefs = model.preferences.borrow();
		ItemsTableModel::new(
			current_collection,
			prefs.paths.software_lists.clone(),
			selection,
			empty_callback,
		)
	};
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

	// now create the "real initial" state, now that we have a model to work with
	let paths = model.preferences.borrow().paths.clone();
	let mame_windowing = if let Some(text) = model.child_window.text() {
		MameWindowing::Attached(text.into())
	} else {
		MameWindowing::Windowed
	};
	let model_weak = Rc::downgrade(&model);
	let state = AppState::new(prefs_path, paths, mame_windowing, args.mame_stderr, move |command| {
		let model = model_weak.upgrade().unwrap();
		handle_command(&model, command);
	});

	// and lets do something with that state; specifically
	// - load the InfoDB (if availble)
	// - start the MAME session (and maybe an InfoDB build in parallel)
	model.update_state(|_| state.activate());

	// initial updates
	update_ui_for_current_history_item(&model);
	update_items_model_for_columns_and_search(&model);

	// and we're done!
	app_window
}

fn menu_item_info(parent_title: Option<&str>, title: &str) -> (Option<AppCommand>, Option<Accelerator>) {
	let (command, accelerator) = match (parent_title, title) {
		// File menu
		(_, "Stop") => (Some(AppCommand::FileStop), None),
		(_, "Pause") => (Some(AppCommand::FilePause), Some("Pause")),
		(_, "Devices and Images...") => (Some(AppCommand::FileDevicesAndImages), None),
		(_, "Quick Load State") => (Some(AppCommand::FileQuickLoadState), Some("F7")),
		(_, "Quick Save State") => (Some(AppCommand::FileQuickLoadState), Some("Shift+F7")),
		(_, "Load State...") => (Some(AppCommand::FileLoadState), Some("Ctrl+F7")),
		(_, "Save State...") => (Some(AppCommand::FileLoadState), Some("Ctrl+Shift+F7")),
		(_, "Save Screenshot...") => (Some(AppCommand::FileSaveScreenshot), Some("F12")),
		(_, "Record Movie...") => (Some(AppCommand::FileRecordMovie), Some("Shift+F12")),
		(_, "Debugger...") => (Some(AppCommand::FileDebugger), None),
		(_, "Soft Reset") => (Some(AppCommand::FileResetSoft), None),
		(_, "Hard Reset") => (Some(AppCommand::FileResetHard), None),
		(_, "Exit") => (Some(AppCommand::FileExit), Some("Ctrl+Alt+X")),

		// Options menu
		(Some("Throttle"), "Increase Speed") => (None, Some("F9")),
		(Some("Throttle"), "Decrease Speed") => (None, Some("F8")),
		(Some("Throttle"), "Warp mode") => (Some(AppCommand::OptionsToggleWarp), Some("F10")),
		(Some("Throttle"), rate) => {
			let rate = rate.strip_suffix('%').unwrap().parse().unwrap();
			(Some(AppCommand::OptionsThrottleRate(rate)), None)
		}
		(_, "Full Screen") => (Some(AppCommand::OptionsToggleFullScreen), Some("F11")),
		(_, "Sound") => (Some(AppCommand::OptionsToggleSound), None),
		(_, "Classic MAME Menu") => (Some(AppCommand::OptionsClassic), None),

		// Settings menu
		(_, "Paths...") => (Some(AppCommand::SettingsPaths(None)), None),
		(Some("Builtin Collections"), col) => {
			let col = BuiltinCollection::from_str(col).unwrap();
			(Some(AppCommand::SettingsToggleBuiltinCollection(col)), None)
		}
		(_, "Reset Settings To Default") => (Some(AppCommand::SettingsReset), None),

		// Help menu
		(_, "Refresh MAME machine info...") => (Some(AppCommand::HelpRefreshInfoDb), None),
		(_, "BletchMAME web site...") => (Some(AppCommand::HelpWebSite), None),
		(_, "About...") => (Some(AppCommand::HelpAbout), None),

		// Anything else
		(_, _) => (None, None),
	};
	event!(LOG_MENUING, parent_title=?parent_title, title=?title, command=?command, accelerator=?accelerator, "menu_item_info");
	(command, accelerator.and_then(accel))
}

fn handle_command(model: &Rc<AppModel>, command: AppCommand) {
	event!(LOG_COMMANDS, command=?&command, "handle_command()");
	match command {
		AppCommand::FileStop => {
			model.issue_command(MameCommand::Stop);
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
				model.issue_command(MameCommand::Resume);
			} else {
				model.issue_command(MameCommand::Pause);
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
				model.app_window_weak.clone(),
				diconfig,
				status_update_channel,
				invoke_command,
			);
			spawn_local(fut).unwrap();
		}
		AppCommand::FileQuickLoadState => {
			let last_save_state = model.state.borrow().last_save_state().unwrap();
			model.issue_command(MameCommand::StateLoad(last_save_state.as_ref()));
		}
		AppCommand::FileQuickSaveState => {
			let last_save_state = model.state.borrow().last_save_state().unwrap();
			model.issue_command(MameCommand::StateSave(last_save_state.as_ref()));
		}
		AppCommand::FileLoadState => {
			let model_clone = model.clone();
			let fut = async move {
				let last_save_state = model_clone.state.borrow().last_save_state();
				let (initial_dir, initial_file) =
					initial_dir_and_file_from_path(last_save_state.as_deref().map(Path::new));

				let title = "Load State";
				let file_types = SAVE_STATE_FILE_TYPES;
				if let Some(filename) =
					load_file_dialog(&model_clone.app_window(), title, file_types, initial_dir, initial_file).await
				{
					model_clone.issue_command(MameCommand::StateLoad(&filename));
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

				let title = "Save State";
				let file_types = SAVE_STATE_FILE_TYPES;
				if let Some(filename) =
					save_file_dialog(&model_clone.app_window(), title, file_types, initial_dir, initial_file).await
				{
					model_clone.issue_command(MameCommand::StateSave(&filename));
					model_clone.update_state(|state| Some(state.set_last_save_state(Some(filename.into()))));
				}
			};
			spawn_local(fut).unwrap();
		}
		AppCommand::FileSaveScreenshot => {
			let model_clone = model.clone();
			let fut = async move {
				let model = model_clone.as_ref();
				let title = "Save Screenshot";
				let file_types = [(None, "png")];
				let initial_file = model.suggested_initial_save_filename("png");
				if let Some(filename) =
					save_file_dialog(&model.app_window(), title, &file_types, None, initial_file.as_deref()).await
				{
					model.issue_command(MameCommand::SaveSnapshot(0, &filename));
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
				model.issue_command(MameCommand::EndRecording);
			} else {
				let model_clone = model.clone();
				let fut = async move {
					let model = model_clone.as_ref();
					let title = "Record Movie";
					let file_types = MovieFormat::all_values()
						.iter()
						.map(MovieFormat::to_string)
						.collect::<Vec<_>>();
					let file_types = file_types.iter().map(|ext| (None, ext.as_str())).collect::<Vec<_>>();
					let initial_file = model.suggested_initial_save_filename(&MovieFormat::default().to_string());
					if let Some(filename) =
						save_file_dialog(&model.app_window(), title, &file_types, None, initial_file.as_deref()).await
					{
						let movie_format = MovieFormat::try_from(Path::new(&filename)).unwrap_or_default();
						model.issue_command(MameCommand::BeginRecording(&filename, movie_format));
					}
				};
				spawn_local(fut).unwrap();
			}
		}
		AppCommand::FileDebugger => {
			model.issue_command(MameCommand::Debugger);
		}
		AppCommand::FileResetSoft => {
			model.issue_command(MameCommand::SoftReset);
		}
		AppCommand::FileResetHard => {
			model.issue_command(MameCommand::HardReset);
		}
		AppCommand::FileExit => {
			model.update_state(AppState::shutdown);
		}
		AppCommand::OptionsThrottleRate(throttle) => {
			model.issue_command(MameCommand::ThrottleRate(throttle));
		}
		AppCommand::OptionsToggleWarp => {
			let is_throttled = model
				.state
				.borrow()
				.status()
				.and_then(|s| s.running.as_ref())
				.map(|r| r.is_throttled)
				.unwrap_or_default();
			model.issue_command(MameCommand::Throttled(!is_throttled));
		}
		AppCommand::OptionsToggleFullScreen => {
			let app_window = model.app_window();
			let window = app_window.window();
			let is_fullscreen = window.is_fullscreen();
			model.preferences.borrow_mut().is_fullscreen = !is_fullscreen;
			window.set_fullscreen(!is_fullscreen);
		}
		AppCommand::OptionsToggleSound => {
			if let Some(sound_attenuation) = model
				.state
				.borrow()
				.status()
				.and_then(|s| s.running.as_ref())
				.map(|r| r.sound_attenuation)
			{
				let is_sound_enabled = sound_attenuation > SOUND_ATTENUATION_OFF;
				let new_attenuation = if is_sound_enabled {
					SOUND_ATTENUATION_OFF
				} else {
					SOUND_ATTENUATION_ON
				};
				model.issue_command(MameCommand::SetAttenuation(new_attenuation));
			}
		}
		AppCommand::OptionsClassic => {
			model.issue_command(MameCommand::ClassicMenu);
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
			let prefs_path = model.state.borrow().prefs_path().to_str().map(|x| x.to_string());
			*prefs = Preferences::fresh(prefs_path);
		}),
		AppCommand::HelpRefreshInfoDb => {
			model.update_state(|state| state.infodb_rebuild());
		}
		AppCommand::HelpWebSite => {
			let _ = open::that("https://www.bletchmame.org");
		}
		AppCommand::HelpAbout => {
			let modal = Modal::new(&model.app_window(), || AboutDialog::new().unwrap());
			modal.launch();
		}
		AppCommand::MameSessionEnded => {
			model.update_state(|state| Some(state.session_ended()));
		}
		AppCommand::MameStatusUpdate(update) => {
			model.update_state(|state| state.status_update(update));
		}
		AppCommand::ErrorMessageBox(message) => {
			let parent = model.app_window().as_weak();
			let fut = async move {
				dialog_message_box::<OkOnly>(parent, "Error", message).await;
			};
			spawn_local(fut).unwrap();
		}
		AppCommand::RunMame {
			machine_name,
			initial_loads,
		} => {
			let initial_loads = initial_loads
				.iter()
				.map(|(dev, arg)| (dev.as_ref(), arg.as_ref()))
				.collect::<Vec<_>>();

			let command = MameCommand::Start {
				machine_name: &machine_name,
				initial_loads: initial_loads.as_slice(),
			};
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
			let parent = model.app_window().as_weak();
			let model_clone = model.clone();
			let fut = async move {
				if let Some(name) = dialog_new_collection(parent, existing_names).await {
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
			let parent = model.app_window().as_weak();
			let model_clone = model.clone();
			let old_name = get_collection_name(&model.preferences.borrow().collections, index).to_string();
			let fut = async move {
				let message = format!("Are you sure you want to delete \"{}\"", old_name);
				if dialog_message_box::<OkCancel>(parent, "Delete", message).await == OkCancel::Ok {
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
			let parent = model.app_window().as_weak();
			let model_clone = model.clone();
			let old_name = get_collection_name(&model.preferences.borrow().collections, index).to_string();
			let fut = async move {
				if let Some(new_name) = dialog_rename_collection(parent, existing_names, old_name).await {
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
			let parent = model.app_window_weak.clone();
			let state = model.state.borrow();
			let image = state
				.status()
				.and_then(|s| s.running.as_ref())
				.unwrap()
				.images
				.iter()
				.find(|x| x.tag == tag)
				.unwrap();
			let format_iter = image.details.formats.iter().map(|f| Format {
				description: &f.description,
				extensions: &f.extensions,
			});
			if let Some(filename) = dialog_load_image(parent, format_iter) {
				let command = AppCommand::LoadImage { tag, filename };
				handle_command(model, command);
			}
		}
		AppCommand::LoadImage { tag, filename } => {
			let loads = [(tag.as_str(), filename.as_str())];
			model.issue_command(MameCommand::LoadImage(&loads));
		}
		AppCommand::UnloadImage { tag } => {
			model.issue_command(MameCommand::UnloadImage(tag.as_str()));
		}
		AppCommand::ConnectToSocketDialog { tag } => {
			let model_clone = model.clone();
			let fut = async move {
				let parent = model_clone.app_window_weak.clone();
				if let Some((hostname, port)) = dialog_connect_to_socket(parent).await {
					let filename = format!("socket.{hostname}:{port}");
					let command = AppCommand::LoadImage { tag, filename };
					handle_command(&model_clone, command);
				}
			};
			spawn_local(fut).unwrap();
		}
		AppCommand::ChangeSlots(changes) => {
			let changes = changes
				.iter()
				.map(|(slot, opt)| (slot.as_str(), opt.as_deref().unwrap_or_default()))
				.collect::<Vec<_>>();
			model.issue_command(MameCommand::ChangeSlots(&changes));
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
				let parent = model_clone.app_window_weak.clone();
				let paths = model_clone.preferences.borrow().paths.clone();
				if let Some(item) = dialog_configure(parent, info_db, item, &paths).await {
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
	};
}

async fn show_paths_dialog(model: Rc<AppModel>, path_type: Option<PathType>) {
	let parent = model.app_window_weak.clone();
	let paths = model.preferences.borrow().paths.clone();
	if let Some(new_paths) = dialog_paths(parent, paths, path_type).await {
		model.modify_prefs(|prefs| prefs.paths = new_paths.into());
	}
}

fn update_menus(model: &AppModel) {
	// calculate properties
	let state = model.state.borrow();
	let build = state.status().map(|s| &s.build);
	let running = state.status().and_then(|s| s.running.as_ref());
	let has_mame_executable = model.preferences.borrow().paths.mame_executable.is_some();
	let is_running = running.is_some();
	let is_paused = running.as_ref().map(|r| r.is_paused).unwrap_or_default();
	let is_throttled = running.as_ref().map(|r| r.is_throttled).unwrap_or_default();
	let throttle_rate = running.as_ref().map(|r| r.throttle_rate);
	let is_sound_enabled = running
		.as_ref()
		.map(|r| r.sound_attenuation > SOUND_ATTENUATION_OFF)
		.unwrap_or_default();
	let can_refresh_info_db = has_mame_executable && !state.is_building_infodb();
	let is_fullscreen = model.app_window().window().is_fullscreen();
	let is_recording = running.as_ref().map(|r| r.is_recording).unwrap_or_default();
	let recording_message = if is_recording {
		"Stop Recording"
	} else {
		"Record Movie..."
	};
	let has_last_save_state = is_running && state.last_save_state().is_some();

	// update the menu bar
	model.app_window().window().with_muda_menu(|menu_bar| {
		menu_bar.update(|parent_title, title| {
			let (command, _) = menu_item_info(parent_title, title);
			let (enabled, checked, text) = match command {
				Some(AppCommand::FileStop) => (Some(is_running), None, None),
				Some(AppCommand::FilePause) => (Some(is_running), Some(is_paused), None),
				Some(AppCommand::FileDevicesAndImages) => (Some(is_running), None, None),
				Some(AppCommand::FileQuickLoadState) => (Some(has_last_save_state), None, None),
				Some(AppCommand::FileQuickSaveState) => (Some(has_last_save_state), None, None),
				Some(AppCommand::FileLoadState) => (Some(is_running), None, None),
				Some(AppCommand::FileSaveState) => (Some(is_running), None, None),
				Some(AppCommand::FileSaveScreenshot) => (Some(is_running), None, None),
				Some(AppCommand::FileRecordMovie) => (Some(is_running), None, Some(recording_message.into())),
				Some(AppCommand::FileDebugger) => (Some(is_running), None, None),
				Some(AppCommand::FileResetSoft) => (Some(is_running), None, None),
				Some(AppCommand::FileResetHard) => (Some(is_running), None, None),
				Some(AppCommand::OptionsThrottleRate(x)) => (Some(is_running), Some(Some(x) == throttle_rate), None),
				Some(AppCommand::OptionsToggleWarp) => (Some(is_running), Some(!is_throttled), None),
				Some(AppCommand::OptionsToggleFullScreen) => (None, Some(is_fullscreen), None),
				Some(AppCommand::OptionsToggleSound) => (Some(is_running), Some(is_sound_enabled), None),
				Some(AppCommand::OptionsClassic) => (Some(is_running), None, None),
				Some(AppCommand::HelpRefreshInfoDb) => (Some(can_refresh_info_db), None, None),
				_ => (None, None, None),
			};

			// factor in the minimum MAME version when deteriming enabled, if available
			let enabled = enabled.map(|e| {
				e && command
					.as_ref()
					.and_then(AppCommand::minimum_mame_version)
					.is_none_or(|a| build.is_some_and(|b| b >= &a))
			});
			MenuItemUpdate { enabled, checked, text }
		})
	});
}

/// updates all UI elements to reflect the current history item
fn update_ui_for_current_history_item(model: &AppModel) {
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

	// update the items view
	model.with_items_table_model(|items_model| {
		items_model.set_current_collection(collection, search, &prefs.current_history_entry().selection);
	});

	drop(prefs);
	update_ui_for_sort_changes(model);
}

fn update_ui_for_sort_changes(model: &AppModel) {
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

	update_items_model_for_columns_and_search(model);
}

fn update_items_model_for_columns_and_search(model: &AppModel) {
	model.with_items_table_model(move |x| {
		let prefs = model.preferences.borrow();
		let entry = prefs.current_history_entry();
		x.set_columns_and_search(&prefs.items_columns, &entry.search, entry.sort_suppressed);
	});
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

fn software_paths_updated(model: &AppModel) {
	let software_list_paths = model.preferences.borrow().paths.software_lists.clone();
	model.with_items_table_model(|x| x.set_software_list_paths(software_list_paths));
}

fn items_set_sorting(model: &Rc<AppModel>, column: i32, order: SortOrder) {
	let column = usize::try_from(column).unwrap();
	let command = AppCommand::ItemsSort(column, order);
	handle_command(model, command);
}
