use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use muda::IsMenuItem;
use muda::Menu;
use muda::MenuEvent;
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

use crate::appcommand::AppCommand;
use crate::collections::add_items_to_existing_folder_collection;
use crate::collections::add_items_to_new_folder_collection;
use crate::collections::get_folder_collection_names;
use crate::collections::get_folder_collections;
use crate::collections::remove_items_from_folder_collection;
use crate::collections::toggle_builtin_collection;
use crate::dialogs::file::file_dialog;
use crate::dialogs::file::PathType;
use crate::dialogs::loading::dialog_load_mame_info;
use crate::dialogs::newcollection::dialog_new_collection;
use crate::dialogs::paths::dialog_paths;
use crate::guiutils::is_context_menu_event;
use crate::guiutils::menuing::accel;
use crate::guiutils::menuing::iterate_menu_items;
use crate::guiutils::menuing::setup_window_menu_bar;
use crate::guiutils::menuing::show_popup_menu;
use crate::guiutils::windowing::with_modal_parent;
use crate::history::History;
use crate::info::InfoDb;
use crate::models::collectionsview::CollectionsViewModel;
use crate::models::itemstable::EmptyReason;
use crate::models::itemstable::ItemsTableModel;
use crate::prefs::BuiltinCollection;
use crate::prefs::Preferences;
use crate::prefs::SortOrder;
use crate::selection::SelectionManager;
use crate::threadlocalbubble::ThreadLocalBubble;
use crate::ui::AboutDialog;
use crate::ui::AppWindow;

const LOG_COMMANDS: bool = false;

type InfoDbSubscriberCallback = Box<dyn Fn(Option<Rc<InfoDb>>, &Preferences)>;

struct AppModel {
	menu_bar: Menu,
	app_window_weak: Weak<AppWindow>,
	preferences: RefCell<Preferences>,
	info_db: RefCell<Option<Rc<InfoDb>>>,
	info_db_subscribers: RefCell<Vec<InfoDbSubscriberCallback>>,
	empty_button_command: RefCell<Option<AppCommand>>,
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
			self.with_collections_view_model(|x| x.update(&prefs.collections));
		}
		if prefs.current_history_entry() != old_prefs.current_history_entry()
			|| prefs.current_collection() != old_prefs.current_collection()
		{
			update_ui_for_current_history_item(self);
		}
		if prefs.items_columns != old_prefs.items_columns {
			update_ui_for_sort_changes(self);
		}
		if prefs.paths != old_prefs.paths {
			if prefs.paths.mame_executable != old_prefs.paths.mame_executable {
				let fut = process_mame_listxml(self.clone());
				spawn_local(fut).unwrap();
			}
			if prefs.paths.software_lists != old_prefs.paths.software_lists {
				software_paths_updated(self);
			}
		}
	}

	pub fn subscribe_to_info_db_changes(&self, func: impl Fn(Option<Rc<InfoDb>>, &Preferences) + 'static) {
		let mut subscribers = self.info_db_subscribers.borrow_mut();
		let func = Box::new(func);
		subscribers.push(func);
	}

	pub fn set_info_db(&self, info_db: Option<InfoDb>) {
		let info_db = info_db.map(Rc::new);
		self.info_db.replace(info_db);

		// borrow prefs
		let prefs = self.preferences.borrow();

		// reset the items view model to reflect the change
		for func in self.info_db_subscribers.borrow().iter() {
			let info_db = self.info_db.borrow().clone();
			func(info_db, &prefs);
		}
	}
}

pub fn create(prefs_path: Option<PathBuf>) -> AppWindow {
	let app_window = AppWindow::new().unwrap();

	let toggle_builtin_menu_items = BuiltinCollection::all_values()
		.iter()
		.map(|x| {
			let id = AppCommand::SettingsToggleBuiltinCollection(*x);
			MenuItem::with_id(id, &format!("{}", x), true, None)
		})
		.collect::<Vec<_>>();
	let toggle_builtin_menu_items = toggle_builtin_menu_items
		.iter()
		.map(|x| x as &dyn IsMenuItem)
		.collect::<Vec<_>>();

	// Menu bar
	#[rustfmt::skip]
	let menu_bar = Menu::with_items(&[
		&Submenu::with_items(
			"File",
			true,
			&[
				&MenuItem::new("Stop", false, None),
				&MenuItem::new("Pause", false, accel("Pause")),
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
					false,
					&[
						&MenuItem::new("Soft Reset", false, None),
						&MenuItem::new("Hard Reset", false, None),
					],
				)
				.unwrap(),
				&MenuItem::with_id(AppCommand::FileExit, "Exit", true, accel("Ctrl+Alt+X"))
			],
		)
		.unwrap(),
		&Submenu::with_items("Settings", true, &[
			&MenuItem::with_id(AppCommand::SettingsPaths, "Paths...", true, None),
			&Submenu::with_items("Builtin Collections", true, &toggle_builtin_menu_items).unwrap(),
			&MenuItem::with_id(AppCommand::SettingsReset, "Reset Settings To Default", true, None)
		]).unwrap(),
		&Submenu::with_items(
			"Help",
			true,
			&[
				&MenuItem::with_id(AppCommand::HelpRefreshInfoDb, "Refresh MAME machine info...", false, None),
				&MenuItem::with_id(AppCommand::HelpWebSite, "BlechMAME web site...", true, None),
				&MenuItem::with_id(AppCommand::HelpAbout, "About...", true, None),
			],
		)
		.unwrap(),
	])
	.unwrap();

	// associate the Menu Bar with our window (looking forward to Slint having first class menuing support)
	setup_window_menu_bar(app_window.window(), &menu_bar);

	// get preferences
	let preferences = Preferences::load(prefs_path.as_ref()).unwrap_or_else(|_| Preferences::fresh(prefs_path));

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
		info_db_subscribers: RefCell::new(Vec::new()),
		empty_button_command: RefCell::new(None),
	};
	let model = Rc::new(model);

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
	model.subscribe_to_info_db_changes(move |info_db, _| items_model_clone.info_db_changed(info_db));
	let items_model_clone = items_model.clone();
	app_window.set_items_model(ModelRc::new(items_model_clone));

	// InfoDB changes
	let collections_view_model_clone = collections_view_model.clone();
	model.subscribe_to_info_db_changes(move |_, prefs| {
		collections_view_model_clone.update(&prefs.collections);
	});

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
	app_window.on_items_sort_ascending(move |index| {
		items_set_sorting(&model_clone, index, SortOrder::Ascending);
	});
	let model_clone = model.clone();
	app_window.on_items_sort_descending(move |index| {
		items_set_sorting(&model_clone, index, SortOrder::Descending);
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
		update_prefs(&model_clone);
		CloseRequestResponse::HideWindow
	});

	// collections popup menus
	let model_clone = model.clone();
	app_window.on_collections_row_pointer_event(move |index, evt, point| {
		if is_context_menu_event(&evt) {
			let index = usize::try_from(index).unwrap();
			if let Some(popup_menu) = model_clone.with_collections_view_model(|x| x.context_commands(index)) {
				let app_window = model_clone.app_window();
				show_popup_menu(app_window.window(), &popup_menu, point);
			}
		}
	});

	// items popup menus
	let model_clone = model.clone();
	app_window.on_items_row_pointer_event(move |index, evt, point| {
		if is_context_menu_event(&evt) {
			let index = usize::try_from(index).unwrap();
			let folder_info = get_folder_collections(&model_clone.preferences.borrow().collections);
			if let Some(popup_menu) = model_clone.with_items_table_model(|x| x.context_commands(index, &folder_info)) {
				let app_window = model_clone.app_window();
				show_popup_menu(app_window.window(), &popup_menu, point);
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

fn handle_command(model: &Rc<AppModel>, command: AppCommand) {
	if LOG_COMMANDS {
		println!("handle_command(): command={:?}", &command);
	}
	match command {
		AppCommand::FileExit => {
			update_prefs(&model.clone());
			quit_event_loop().unwrap()
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
			let dialog = with_modal_parent(&model.app_window(), || AboutDialog::new().unwrap());
			dialog.show().unwrap();
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
					// the colleciton is being removed; we need to remove any entries that
					// might be referenced
					prefs.purge_stray_entries();
				}
			});
		}
		AppCommand::ChoosePath(path_type) => {
			choose_path(model, path_type);
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
		let new_paths = Rc::new(new_paths);
		model.modify_prefs(|prefs| prefs.paths = new_paths);
	}
}

fn update(model: &AppModel) {
	// calculate properties
	let has_mame_executable = model.preferences.borrow().paths.mame_executable.is_some();

	// update the menu bar
	for menu_item in iterate_menu_items(&model.menu_bar) {
		if let Ok(AppCommand::HelpRefreshInfoDb) = AppCommand::try_from(menu_item.id()) {
			menu_item.set_enabled(has_mame_executable)
		}
	}

	// update history buttons
	update_ui_for_current_history_item(model);
	update_items_model_for_columns_and_search(model);
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
		match path_type {
			PathType::MameExecutable => {
				paths.mame_executable = Some(path);
			}
			PathType::SoftwareLists => {
				paths.software_lists = vec![path];
			}
		};
		prefs.paths = Rc::new(paths);
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
