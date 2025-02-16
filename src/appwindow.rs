use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use muda::CheckMenuItem;
use muda::IsMenuItem;
use muda::Menu;
use muda::MenuEvent;
use muda::MenuId;
use muda::MenuItem;
use muda::PredefinedMenuItem;
use muda::Submenu;
use slint::invoke_from_event_loop;
use slint::quit_event_loop;
use slint::spawn_local;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::LogicalPosition;
use slint::LogicalSize;
use slint::Model;
use slint::ModelRc;
use slint::SharedString;
use slint::TableColumn;
use slint::VecModel;
use slint::Weak;
use tracing::event;
use tracing::Level;

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
use crate::dialogs::devimages::dialog_devices_and_images;
use crate::dialogs::image::dialog_load_image;
use crate::dialogs::messagebox::dialog_message_box;
use crate::dialogs::messagebox::OkCancel;
use crate::dialogs::messagebox::OkOnly;
use crate::dialogs::namecollection::dialog_new_collection;
use crate::dialogs::namecollection::dialog_rename_collection;
use crate::dialogs::paths::dialog_paths;
use crate::dialogs::socket::dialog_connect_to_socket;
use crate::guiutils::is_context_menu_event;
use crate::guiutils::menuing::accel;
use crate::guiutils::menuing::MenuExt;
use crate::guiutils::menuing::MenuItemUpdate;
use crate::guiutils::modal::Modal;
use crate::guiutils::MenuingType;
use crate::history::History;
use crate::models::collectionsview::CollectionsViewModel;
use crate::models::itemstable::EmptyReason;
use crate::models::itemstable::ItemsTableModel;
use crate::platform::WindowExt;
use crate::prefs::pathtype::PathType;
use crate::prefs::BuiltinCollection;
use crate::prefs::Preferences;
use crate::prefs::SortOrder;
use crate::runtime::command::MameCommand;
use crate::runtime::MameStderr;
use crate::runtime::MameWindowing;
use crate::selection::SelectionManager;
use crate::status::Status;
use crate::threadlocalbubble::ThreadLocalBubble;
use crate::ui::AboutDialog;
use crate::ui::AppWindow;
use crate::ui::ReportIssue;

const LOG_COMMANDS: Level = Level::DEBUG;
const LOG_PREFS: Level = Level::DEBUG;

const SOUND_ATTENUATION_OFF: i32 = -32;
const SOUND_ATTENUATION_ON: i32 = 0;

/// Arguments to the application (derivative from the command line); almost all of this
/// are power user features or diagnostics
#[derive(Debug)]
pub struct AppArgs {
	pub prefs_path: PathBuf,
	pub mame_stderr: MameStderr,
	pub menuing_type: MenuingType,
}

struct AppModel {
	menu_bar: Menu,
	menuing_type: MenuingType,
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
		let _ = self.preferences.borrow().save();

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

	pub fn show_popup_menu(&self, popup_menu: Menu, position: LogicalPosition) {
		let app_window = self.app_window();
		match self.menuing_type {
			MenuingType::Native => {
				app_window.window().show_popup_menu(&popup_menu, position);
			}
			MenuingType::Slint => {
				let entries = popup_menu.slint_menu_entries(None);
				app_window.invoke_show_context_menu(entries, position);
			}
		}
	}

	pub fn issue_command(&self, command: MameCommand<'_>) {
		self.state.borrow().issue_command(command);
	}
}

pub fn create(args: AppArgs) -> AppWindow {
	let app_window = AppWindow::new().unwrap();

	// child window for MAME to attach to
	let child_window =
		ChildWindow::new(app_window.window()).unwrap_or_else(|e| panic!("Failed to create child window: {e:?}"));

	// create the menu bar
	let menu_bar = create_menu_bar();

	// get preferences
	let prefs_path = args.prefs_path;
	let preferences = Preferences::load(&prefs_path)
		.ok()
		.flatten()
		.unwrap_or_else(|| Preferences::fresh(prefs_path));

	// update window preferences
	if let Some(window_size) = &preferences.window_size {
		let physical_size = LogicalSize::from(*window_size).to_physical(app_window.window().scale_factor());
		app_window.window().set_size(physical_size);
	}

	// create the model
	let model = AppModel {
		menu_bar,
		menuing_type: args.menuing_type,
		app_window_weak: app_window.as_weak(),
		preferences: RefCell::new(preferences),
		state: RefCell::new(AppState::bogus()),
		status_changed_channel: Channel::default(),
		child_window,
	};
	let model = Rc::new(model);

	// attach the menu bar (either natively or with an approximation using Slint); looking forward to Slint having first class menuing support
	match args.menuing_type {
		MenuingType::Native => {
			// attach a native menu bar
			app_window
				.window()
				.attach_menu_bar(&model.menu_bar)
				.unwrap_or_else(|e| panic!("Failed to attach menu bar: {e:?}"));
		}
		MenuingType::Slint => {
			// set up Slint menu bar by proxying muda menu bar
			app_window.set_menubar_entries(model.menu_bar.slint_menu_entries(None));
			let model_clone = model.clone();
			app_window.on_menubar_sub_menu_selected(move |entry| model_clone.menu_bar.slint_menu_entries(Some(&entry)));
			let model_clone = model.clone();
			app_window.on_menu_entry_activated(move |entry| {
				let id = MenuId::from(&entry.id);
				if let Ok(command) = AppCommand::try_from(&id) {
					handle_command(&model_clone, command);
				}
			});
		}
	}

	// create a repeating future that will update the child window forever
	let model_weak = Rc::downgrade(&model);
	app_window.on_size_changed(move || {
		if let Some(model) = model_weak.upgrade() {
			// set the child window size
			let menubar_height = model.app_window().invoke_menubar_height();
			model.child_window.update(model.app_window().window(), menubar_height);
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

	// set up menu handler
	let packet = ThreadLocalBubble::new(model.clone());
	MenuEvent::set_event_handler(Some(move |menu_event: MenuEvent| {
		if let Ok(command) = AppCommand::try_from(&menu_event.id) {
			let packet = packet.clone();
			invoke_from_event_loop(move || {
				let model = packet.unwrap();
				handle_command(&model, command);
			})
			.unwrap();
		}
	}));

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
			if let Some(popup_menu) = model_clone.with_collections_view_model(|x| x.context_commands(index)) {
				model_clone.show_popup_menu(popup_menu, position);
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
			if let Some(popup_menu) =
				model_clone.with_items_table_model(|x| x.context_commands(index, &folder_info, has_mame_initialized))
			{
				model_clone.show_popup_menu(popup_menu, position);
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
	let prefs_path = model.preferences.borrow().prefs_path.clone();
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

fn create_menu_bar() -> Menu {
	fn to_menu_item_ref_vec(items: &[impl IsMenuItem]) -> Vec<&dyn IsMenuItem> {
		items.iter().map(|x| x as &dyn IsMenuItem).collect::<Vec<_>>()
	}

	let toggle_builtin_menu_items = BuiltinCollection::all_values()
		.iter()
		.map(|x| {
			let id = AppCommand::SettingsToggleBuiltinCollection(*x);
			MenuItem::with_id(id, format!("{}", x), true, None)
		})
		.collect::<Vec<_>>();
	let toggle_builtin_menu_items = to_menu_item_ref_vec(&toggle_builtin_menu_items);

	#[rustfmt::skip]
	let menu_bar = Menu::with_items(&[
		&Submenu::with_items(
			"File",
			true,
			&[
				&MenuItem::with_id(AppCommand::FileStop, "Stop", false, None),
				&CheckMenuItem::with_id(AppCommand::FilePause, "Pause", false, false, accel("Pause")),
				&PredefinedMenuItem::separator(),
				&MenuItem::with_id(AppCommand::FileDevicesAndImages,"Devices and Images...", false, None),
				&PredefinedMenuItem::separator(),
				&MenuItem::new("Quick Load State", false, accel("F7")),
				&MenuItem::new("Quick Save State", false, accel("Shift+F7")),
				&MenuItem::new("Load State...", false, accel("Ctrl+F7")),
				&MenuItem::new("Save State...", false, accel("Ctrl+Shift+F7")),
				&PredefinedMenuItem::separator(),
				&MenuItem::new("Debugger...", false, None),
				&Submenu::with_items(
					"Reset",
					true,
					&[
						&MenuItem::with_id(AppCommand::FileResetSoft, "Soft Reset", false, None),
						&MenuItem::with_id(AppCommand::FileResetHard,"Hard Reset", false, None),
					],
				)
				.unwrap(),
				&MenuItem::with_id(AppCommand::FileExit, "Exit", true, accel("Ctrl+Alt+X")),
			],
		)
		.unwrap(),
		&Submenu::with_items(
			"Options",
			true,
			&[
				&Submenu::with_items(
					"Throttle",
					true,
					&[
						&CheckMenuItem::with_id(AppCommand::OptionsThrottleRate(10.0), "1000%", false, false, None),
						&CheckMenuItem::with_id(AppCommand::OptionsThrottleRate(5.0), "500%", false, false, None),
						&CheckMenuItem::with_id(AppCommand::OptionsThrottleRate(2.0), "200%", false, false, None),
						&CheckMenuItem::with_id(AppCommand::OptionsThrottleRate(1.0), "100%", false, false, None),
						&CheckMenuItem::with_id(AppCommand::OptionsThrottleRate(0.5), "50%", false, false, None),
						&CheckMenuItem::with_id(AppCommand::OptionsThrottleRate(0.2), "20%", false, false, None),
						&CheckMenuItem::with_id(AppCommand::OptionsThrottleRate(0.1), "10%", false, false, None),
						&PredefinedMenuItem::separator(),
						&MenuItem::new("Increase Speed", false, accel("F9")),
						&MenuItem::new("Decrease Speed", false, accel("F8")),
						&CheckMenuItem::with_id(AppCommand::OptionsToggleWarp, "Warp mode", false, false, accel("F10")),
					],
				)
				.unwrap(),
				&Submenu::with_items(
					"Frame Skip",
					false,
					&[
						&MenuItem::new("Auto", false, None),
						&MenuItem::new("0", false, None),
						&MenuItem::new("1", false, None),
						&MenuItem::new("2", false, None),
						&MenuItem::new("3", false, None),
						&MenuItem::new("4", false, None),
						&MenuItem::new("5", false, None),
						&MenuItem::new("6", false, None),
						&MenuItem::new("7", false, None),
						&MenuItem::new("8", false, None),
						&MenuItem::new("9", false, None),
						&MenuItem::new("10", false, None),
					],
				)
				.unwrap(),
				&MenuItem::new("Full Screen", false, accel("F11")),
				&CheckMenuItem::with_id(AppCommand::OptionsToggleSound, "Sound", false, false,None),
				&MenuItem::new("Cheats...", false, None),
				&MenuItem::with_id(AppCommand::OptionsClassic,"Classic MAME Menu", false, None),
			],
		)
		.unwrap(),
		&Submenu::with_items(
			"Settings",
			true,
			&[
				&MenuItem::new("Joysticks and Controllers...", false, None),
				&MenuItem::new("Keyboard...", false, None),
				&MenuItem::new("Miscellaneous Input...", false, None),
				&MenuItem::new("Configuration...", false, None),
				&MenuItem::new("DIP Switches...", false, None),
				&PredefinedMenuItem::separator(),
				&MenuItem::with_id(AppCommand::SettingsPaths(None), "Paths...", true, None),
				&Submenu::with_items("Builtin Collections", true, &toggle_builtin_menu_items).unwrap(),
				&MenuItem::with_id(AppCommand::SettingsReset, "Reset Settings To Default", true, None),
				&MenuItem::new("Import MAME INI...", false, None),
			],
		)
		.unwrap(),
		&Submenu::with_items(
			"Help",
			true,
			&[
				&MenuItem::with_id(AppCommand::HelpRefreshInfoDb, "Refresh MAME machine info...", false, None),
				&MenuItem::with_id(AppCommand::HelpWebSite, "BletchMAME web site...", true, None),
				&MenuItem::with_id(AppCommand::HelpAbout, "About...", true, None),
			],
		)
		.unwrap(),
	])
	.unwrap();

	menu_bar
}

fn handle_command(model: &Rc<AppModel>, command: AppCommand) {
	event!(LOG_COMMANDS, "handle_command(): command={:?}", &command);
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
				model.menuing_type,
			);
			spawn_local(fut).unwrap();
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
			*prefs = Preferences::fresh(prefs.prefs_path.clone());
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
			if let Some(filename) = dialog_load_image(parent, image) {
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

	// update the menu bar
	model.menu_bar.update(|id| {
		let command = AppCommand::try_from(id);
		let (enabled, checked) = match command {
			Ok(AppCommand::FileStop) => (Some(is_running), None),
			Ok(AppCommand::FilePause) => (Some(is_running), Some(is_paused)),
			Ok(AppCommand::FileDevicesAndImages) => (Some(is_running), None),
			Ok(AppCommand::FileResetSoft) => (Some(is_running), None),
			Ok(AppCommand::FileResetHard) => (Some(is_running), None),
			Ok(AppCommand::OptionsThrottleRate(x)) => (Some(is_running), Some(Some(x) == throttle_rate)),
			Ok(AppCommand::OptionsToggleWarp) => (Some(is_running), Some(!is_throttled)),
			Ok(AppCommand::OptionsToggleSound) => (Some(is_running), Some(is_sound_enabled)),
			Ok(AppCommand::OptionsClassic) => (Some(is_running), None),
			Ok(AppCommand::HelpRefreshInfoDb) => (Some(can_refresh_info_db), None),
			_ => (None, None),
		};

		// factor in the minimum MAME version when deteriming enabled, if available
		let enabled = enabled.map(|e| {
			e && command
				.as_ref()
				.ok()
				.and_then(AppCommand::minimum_mame_version)
				.is_none_or(|a| build.is_some_and(|b| b >= &a))
		});
		MenuItemUpdate { enabled, checked }
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

#[cfg(test)]
mod test {
	use std::convert::Infallible;
	use std::ops::ControlFlow;

	use crate::appcommand::AppCommand;
	use crate::guiutils::menuing::MenuExt;

	#[test]
	fn create_menu_bar() {
		let menu_bar = super::create_menu_bar();
		menu_bar.visit((), |_, item| {
			if let Ok(command) = AppCommand::try_from(item.id()) {
				let _ = command.minimum_mame_version();
			}
			ControlFlow::<Infallible>::Continue(())
		});
	}
}
