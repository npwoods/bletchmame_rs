use std::cell::Cell;
use std::cell::RefCell;
use std::iter::once;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

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
use slint::LogicalSize;
use slint::Model;
use slint::ModelRc;
use slint::SharedString;
use slint::TableColumn;
use slint::VecModel;
use slint::Weak;
use strum::EnumString;
use tracing::event;
use tracing::Level;

use crate::appcommand::AppCommand;
use crate::collections::add_items_to_existing_folder_collection;
use crate::collections::add_items_to_new_folder_collection;
use crate::collections::get_collection_name;
use crate::collections::get_folder_collection_names;
use crate::collections::get_folder_collections;
use crate::collections::remove_items_from_folder_collection;
use crate::collections::toggle_builtin_collection;
use crate::dialogs::file::file_dialog;
use crate::dialogs::file::PathType;
use crate::dialogs::loading::dialog_load_mame_info;
use crate::dialogs::messagebox::dialog_message_box;
use crate::dialogs::messagebox::OkCancel;
use crate::dialogs::messagebox::OkOnly;
use crate::dialogs::namecollection::dialog_new_collection;
use crate::dialogs::namecollection::dialog_rename_collection;
use crate::dialogs::paths::dialog_paths;
use crate::guiutils::is_context_menu_event;
use crate::guiutils::menuing::accel;
use crate::guiutils::menuing::MenuExt;
use crate::guiutils::menuing::MenuItemUpdate;
use crate::guiutils::modal::Modal;
use crate::history::History;
use crate::info::InfoDb;
use crate::models::collectionsview::CollectionsViewModel;
use crate::models::itemstable::EmptyReason;
use crate::models::itemstable::ItemsTableModel;
use crate::platform::ChildWindow;
use crate::platform::WindowExt;
use crate::prefs::BuiltinCollection;
use crate::prefs::Preferences;
use crate::prefs::SortOrder;
use crate::runtime::controller::MameController;
use crate::runtime::MameCommand;
use crate::runtime::MameEvent;
use crate::runtime::MameStderr;
use crate::runtime::MameWindowing;
use crate::selection::SelectionManager;
use crate::status::Status;
use crate::threadlocalbubble::ThreadLocalBubble;
use crate::ui::AboutDialog;
use crate::ui::AppWindow;

const LOG_COMMANDS: Level = Level::DEBUG;
const LOG_PREFS: Level = Level::DEBUG;
const LOG_PINGING: Level = Level::TRACE;

const SOUND_ATTENUATION_OFF: i32 = -32;
const SOUND_ATTENUATION_ON: i32 = 0;

/// Arguments to the application (derivative from the command line); almost all of this
/// are power user features or diagnostics
#[derive(Debug)]
pub struct AppArgs {
	pub prefs_path: Option<PathBuf>,
	pub mame_stderr: MameStderr,
	pub menuing_type: MenuingType,
}

#[derive(Debug, EnumString)]
#[strum(ascii_case_insensitive)]
pub enum MenuingType {
	Native,
	Slint,
}

struct AppModel {
	menu_bar: Menu,
	app_window_weak: Weak<AppWindow>,
	preferences: RefCell<Preferences>,
	info_db: RefCell<Option<Rc<InfoDb>>>,
	empty_button_command: RefCell<Option<AppCommand>>,
	mame_controller: MameController,
	running_state: Cell<MameRunningState>,
	running_status: RefCell<Status>,
	child_window: ChildWindow,
	build_skew_state: Cell<BuildSkewState>,
}

#[derive(Copy, Clone, Debug)]
enum MameRunningState {
	Normal,
	Bouncing,
	ShuttingDown,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BuildSkewState {
	Normal,
	MamePathChanged,
	FoundSkew,
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
		if prefs.collections != old_prefs.collections {
			event!(LOG_PREFS, "modify_prefs(): prefs.collection changed");
			let info_db = self.info_db.borrow().clone();
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
		if prefs.paths != old_prefs.paths {
			if self.mame_controller.has_session() {
				self.mame_controller.issue_command(MameCommand::Exit);
				self.running_state.set(MameRunningState::Bouncing);
			}
			if prefs.paths.mame_executable != old_prefs.paths.mame_executable {
				event!(LOG_PREFS, "modify_prefs(): paths.mame_executable changed");
				self.build_skew_state.set(BuildSkewState::MamePathChanged);

				// refresh MAME InfoDB if we now have an exectuable
				if prefs.paths.mame_executable.is_some() && !self.mame_controller.has_session() {
					handle_command(self, AppCommand::HelpRefreshInfoDb);
				}
			}
			if prefs.paths.software_lists != old_prefs.paths.software_lists {
				event!(LOG_PREFS, "modify_prefs(): paths.software_lists changed");
				software_paths_updated(self);
			}
		}
	}

	pub fn set_info_db(&self, info_db: Option<InfoDb>) {
		let info_db = info_db.map(Rc::new);
		self.info_db.replace(info_db.clone());

		self.with_items_table_model(|items_model| {
			let info_db = info_db.clone();
			items_model.info_db_changed(info_db);
		});
		self.with_collections_view_model(|collections_model| {
			let prefs = self.preferences.borrow();
			let info_db = info_db.clone();
			collections_model.update(info_db, &prefs.collections);
		});
		self.reset_mame_controller(info_db.is_some());
	}

	pub fn update_from_status(&self) {
		let status = self.running_status.borrow();

		// machine description
		let machine_desc = status
			.running
			.as_ref()
			.map(|x| x.machine_name.as_str())
			.unwrap_or_default()
			.into();
		self.app_window().set_running_machine_description(machine_desc);

		// child window visibility
		self.child_window.set_visible(status.running.is_some());

		update_menus(self);
	}

	pub fn reset_mame_controller(&self, is_enabled: bool) {
		let windowing = if let Some(text) = self.child_window.text() {
			MameWindowing::Attached(text)
		} else {
			MameWindowing::Windowed
		};

		self.mame_controller
			.reset(is_enabled.then_some(&self.preferences.borrow().paths), &windowing);
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
	let preferences = Preferences::load(prefs_path.as_ref())
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
		app_window_weak: app_window.as_weak(),
		preferences: RefCell::new(preferences),
		info_db: RefCell::new(None),
		empty_button_command: RefCell::new(None),
		mame_controller: MameController::new(args.mame_stderr),
		running_state: Cell::new(MameRunningState::Normal),
		running_status: RefCell::new(Status::default()),
		child_window,
		build_skew_state: Cell::new(BuildSkewState::Normal),
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

	// set up a callback for MAME events
	let bubble = ThreadLocalBubble::new(model.clone());
	model.mame_controller.set_event_callback(move |event| {
		let bubble = bubble.clone();
		invoke_from_event_loop(move || {
			let model = bubble.unwrap();
			let command = match event {
				MameEvent::SessionStarted => AppCommand::MameSessionStarted,
				MameEvent::SessionEnded => AppCommand::MameSessionEnded,
				MameEvent::Error(e) => AppCommand::ErrorMessageBox(format!("{e:?}")),
				MameEvent::StatusUpdate(update) => AppCommand::MameStatusUpdate(update),
			};
			handle_command(&model, command);
		})
		.unwrap();
	});

	// create a repeating future that will ping forever
	let fut = ping_callback(Rc::downgrade(&model));
	spawn_local(fut).unwrap();

	// the "empty reason action" button
	let model_clone = model.clone();
	app_window.on_empty_action_clicked(move || {
		let command = model_clone
			.empty_button_command
			.borrow()
			.clone()
			.expect("Button should not be clickable if None");
		handle_command(&model_clone, command);
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
	app_window.on_collections_row_pointer_event(move |index, evt, point| {
		if is_context_menu_event(&evt) {
			let index = usize::try_from(index).ok();
			if let Some(popup_menu) = model_clone.with_collections_view_model(|x| x.context_commands(index)) {
				let app_window = model_clone.app_window();
				app_window.window().show_popup_menu(&popup_menu, point);
			}
		}
	});

	// items popup menus
	let model_clone = model.clone();
	app_window.on_items_row_pointer_event(move |index, evt, point| {
		if is_context_menu_event(&evt) {
			let index = usize::try_from(index).unwrap();
			let folder_info = get_folder_collections(&model_clone.preferences.borrow().collections);
			let has_mame_initialized = model_clone.running_status.borrow().has_initialized;
			if let Some(popup_menu) =
				model_clone.with_items_table_model(|x| x.context_commands(index, &folder_info, has_mame_initialized))
			{
				let app_window = model_clone.app_window();
				app_window.window().show_popup_menu(&popup_menu, point);
			}
		}
	});

	// spawn an effort to try to load MAME info from persisted data
	let model_clone = model.clone();
	spawn_local(try_load_persisted_info_db(model_clone)).unwrap();

	// initial update of the model; kick off the process of loading InfoDB and return
	update(&model);
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
				&MenuItem::new("Devices and Images...", false, None),
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
				&MenuItem::with_id(AppCommand::SettingsPaths, "Paths...", true, None),
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
			model.mame_controller.issue_command(MameCommand::Stop);
		}
		AppCommand::FilePause => {
			let is_paused = model
				.running_status
				.borrow()
				.running
				.as_ref()
				.map(|r| r.is_paused)
				.unwrap_or_default();
			if is_paused {
				model.mame_controller.issue_command(MameCommand::Resume);
			} else {
				model.mame_controller.issue_command(MameCommand::Pause);
			}
		}
		AppCommand::FileResetSoft => {
			model.mame_controller.issue_command(MameCommand::SoftReset);
		}
		AppCommand::FileResetHard => {
			model.mame_controller.issue_command(MameCommand::HardReset);
		}
		AppCommand::FileExit => {
			if model.mame_controller.has_session() {
				model.mame_controller.issue_command(MameCommand::Exit);
				model.running_state.set(MameRunningState::ShuttingDown);
			} else {
				handle_command(model, AppCommand::Shutdown);
			}
		}
		AppCommand::OptionsThrottleRate(throttle) => {
			model.mame_controller.issue_command(MameCommand::ThrottleRate(throttle));
		}
		AppCommand::OptionsToggleWarp => {
			let is_throttled = model
				.running_status
				.borrow()
				.running
				.as_ref()
				.map(|r| r.is_throttled)
				.unwrap_or_default();
			model
				.mame_controller
				.issue_command(MameCommand::Throttled(!is_throttled));
		}
		AppCommand::OptionsToggleSound => {
			if let Some(sound_attenuation) = model
				.running_status
				.borrow()
				.running
				.as_ref()
				.map(|r| r.sound_attenuation)
			{
				let is_sound_enabled = sound_attenuation > SOUND_ATTENUATION_OFF;
				let new_attenuation = if is_sound_enabled {
					SOUND_ATTENUATION_OFF
				} else {
					SOUND_ATTENUATION_ON
				};
				model
					.mame_controller
					.issue_command(MameCommand::SetAttenuation(new_attenuation));
			}
		}
		AppCommand::SettingsPaths => {
			let fut = show_paths_dialog(model.clone());
			spawn_local(fut).unwrap();
		}
		AppCommand::SettingsToggleBuiltinCollection(col) => {
			model.modify_prefs(|prefs| {
				toggle_builtin_collection(&mut prefs.collections, col);
			});
		}
		AppCommand::SettingsReset => model.modify_prefs(|prefs| {
			let prefs_path = prefs.prefs_path.take();
			*prefs = Preferences::fresh(prefs_path);
		}),
		AppCommand::HelpRefreshInfoDb => {
			let model = model.clone();
			spawn_local(process_mame_listxml(model)).unwrap();
		}
		AppCommand::HelpWebSite => {
			let _ = open::that("https://www.bletchmame.org");
		}
		AppCommand::HelpAbout => {
			let modal = Modal::new(&model.app_window(), || AboutDialog::new().unwrap());
			modal.launch();
		}
		AppCommand::MameSessionStarted => {
			// do nothing
		}
		AppCommand::MameSessionEnded => {
			let shutting_down = {
				let (is_enabled, shutting_down) = match model.running_state.get() {
					MameRunningState::Normal => (false, false),
					MameRunningState::Bouncing => (true, false),
					MameRunningState::ShuttingDown => (false, true),
				};
				model.reset_mame_controller(is_enabled);
				model.running_state.set(MameRunningState::Normal);
				*model.running_status.borrow_mut() = Status::default();
				model.update_from_status();
				shutting_down
			};
			if shutting_down {
				handle_command(model, AppCommand::Shutdown);
			}
		}
		AppCommand::MameStatusUpdate(update) => {
			model.running_status.borrow_mut().merge(update);
			model.update_from_status();

			// check for build skew
			let next_build_skew_state = next_build_skew_state(
				&model.running_status.borrow(),
				model.info_db.borrow().as_deref(),
				model.build_skew_state.get(),
			);
			let need_to_refresh = model.build_skew_state.get() != BuildSkewState::FoundSkew
				&& next_build_skew_state == BuildSkewState::FoundSkew;
			model.build_skew_state.set(next_build_skew_state);
			if need_to_refresh {
				handle_command(model, AppCommand::HelpRefreshInfoDb);
			}
		}
		AppCommand::MamePing => {
			model.mame_controller.issue_command(MameCommand::Ping);
		}
		AppCommand::ErrorMessageBox(message) => {
			let parent = model.app_window().as_weak();
			let fut = async move {
				dialog_message_box::<OkOnly>(parent, "Error", message).await;
			};
			spawn_local(fut).unwrap();
		}
		AppCommand::Shutdown { .. } => {
			update_prefs(&model.clone());
			quit_event_loop().unwrap()
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
			model.mame_controller.issue_command(command);
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
		AppCommand::ChoosePath(path_type) => {
			choose_path(model, path_type);
		}
		AppCommand::BookmarkCurrentCollection => {
			let (collection, _) = model.preferences.borrow().current_collection();
			model.modify_prefs(|prefs| {
				prefs.collections.push(collection);
			})
		}
	};
}

async fn try_load_persisted_info_db(model: Rc<AppModel>) {
	// load MAME info from persisted data
	let info_db_result = {
		let prefs = model.preferences.borrow();
		let Some(mame_executable_path) = prefs.paths.mame_executable.as_ref() else {
			return;
		};
		InfoDb::load(prefs.prefs_path.as_ref(), mame_executable_path)
	};

	// so... we did indeed try to load the InfoDb... but did we succeed?
	if let Ok(info_db) = info_db_result {
		// we did!  set it up
		model.set_info_db(Some(info_db));
		update(&model);
	} else {
		// we errored for whatever reason; kick off a process to read it
		process_mame_listxml(model).await;
	}
}

/// loads MAME by launching `mame -listxml`
async fn process_mame_listxml(model: Rc<AppModel>) {
	// identify the (optional) MAME executable (which can be passed to us or in preferences)
	let mame_executable = model.preferences.borrow().paths.mame_executable.clone();

	// do we have a MAME executable?  if so show the dialog
	let info_db = if let Some(mame_executable) = mame_executable {
		// present the loading dialog
		let Some(info_db) = dialog_load_mame_info(model.app_window().as_weak(), &mame_executable).await else {
			return; // cancelled or errored
		};

		// the processing succeeded; save the Info DB
		let _ = {
			let prefs = model.preferences.borrow();
			info_db.save(prefs.prefs_path.as_ref(), &mame_executable)
		};

		// lastly we have an Info DB
		Some(info_db)
	} else {
		// no executable!  no Info DB I guess :-|
		None
	};

	// set the model to use the new Info DB
	model.set_info_db(info_db);

	// and update all the things
	update(&model);
}

async fn show_paths_dialog(model: Rc<AppModel>) {
	let parent = model.app_window_weak.clone();
	let paths = model.preferences.borrow().paths.clone();
	if let Some(new_paths) = dialog_paths(parent, paths).await {
		model.modify_prefs(|prefs| prefs.paths = new_paths.into());
	}
}

fn update(model: &AppModel) {
	update_menus(model);
	update_ui_for_current_history_item(model);
	update_items_model_for_columns_and_search(model);
}

fn update_menus(model: &AppModel) {
	// calculate properties
	let running_status = model.running_status.borrow();
	let has_mame_executable = model.preferences.borrow().paths.mame_executable.is_some();
	let is_running = running_status.running.is_some();
	let is_paused = running_status.running.as_ref().map(|r| r.is_paused).unwrap_or_default();
	let is_throttled = running_status
		.running
		.as_ref()
		.map(|r| r.is_throttled)
		.unwrap_or_default();
	let throttle_rate = running_status.running.as_ref().map(|r| r.throttle_rate);
	let is_sound_enabled = running_status
		.running
		.as_ref()
		.map(|r| r.sound_attenuation > SOUND_ATTENUATION_OFF)
		.unwrap_or_default();

	// update the menu bar
	model.menu_bar.update(|id| {
		let (enabled, checked) = match AppCommand::try_from(id) {
			Ok(AppCommand::HelpRefreshInfoDb) => (Some(has_mame_executable), None),
			Ok(AppCommand::FileStop) => (Some(is_running), None),
			Ok(AppCommand::FilePause) => (Some(is_running), Some(is_paused)),
			Ok(AppCommand::FileResetSoft) => (Some(is_running), None),
			Ok(AppCommand::FileResetHard) => (Some(is_running), None),
			Ok(AppCommand::OptionsThrottleRate(x)) => (Some(is_running), Some(Some(x) == throttle_rate)),
			Ok(AppCommand::OptionsToggleWarp) => (Some(is_running), Some(!is_throttled)),
			Ok(AppCommand::OptionsToggleSound) => (Some(is_running), Some(is_sound_enabled)),
			_ => (None, None),
		};
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
		.info_db
		.borrow()
		.as_ref()
		.map(|info_db| collection.description(info_db.as_ref()))
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
	let (button_command, button_text) = empty_reason.and_then(|x| x.action()).unzip();
	let button_text = button_text.unwrap_or_default().into();
	app_window.set_is_empty(empty_reason.is_some());
	app_window.set_is_empty_reason(reason_string);
	app_window.set_is_empty_button_text(button_text);
	model.empty_button_command.replace(button_command);
}

fn choose_path(model: &Rc<AppModel>, path_type: PathType) {
	// open the file dialog
	let Some(path) = file_dialog(&model.app_window(), path_type) else {
		return;
	};

	// and respond to the change
	model.modify_prefs(|prefs| {
		let mut paths = (*prefs.paths).clone();
		PathType::store_in_prefs_paths(&mut paths, path_type, once(path));
		prefs.paths = paths.into();
	});
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

fn next_build_skew_state(status: &Status, info_db: Option<&InfoDb>, last: BuildSkewState) -> BuildSkewState {
	// not having an InfoDb but receiving status updates is a very degenerate scenario
	let Some(info_db) = info_db else {
		return last;
	};

	let has_skew = if let Some(build) = status.build.as_ref() {
		build != info_db.build()
	} else {
		last != BuildSkewState::Normal
	};
	if has_skew {
		BuildSkewState::FoundSkew
	} else {
		BuildSkewState::Normal
	}
}

async fn ping_callback(model_weak: std::rc::Weak<AppModel>) {
	// we really should be turning the timer on and off depending on what is running
	while let Some(model) = model_weak.upgrade() {
		event!(LOG_PINGING, "ping_callback(): pinging");

		// are we running?
		let is_running = model.running_status.borrow().running.is_some();

		// set the child window size
		model.child_window.update(model.app_window().window());

		// send a ping command (if we're running)
		if is_running && model.mame_controller.is_queue_empty() {
			handle_command(&model, AppCommand::MamePing);
		}
		drop(model);
		tokio::time::sleep(Duration::from_secs(1)).await;
	}
	event!(LOG_PINGING, "ping_callback(): exiting");
}
