use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use muda::Menu;
use muda::MenuEvent;
use muda::MenuItem;
use muda::PredefinedMenuItem;
use muda::Submenu;
use rfd::FileDialog;
use serde::Deserialize;
use serde::Serialize;
use slint::invoke_from_event_loop;
use slint::platform::PointerEventButton;
use slint::quit_event_loop;
use slint::spawn_local;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::LogicalSize;
use slint::Model;
use slint::ModelRc;
use slint::Weak;

use crate::dialogs::builtincollections::dialog_builtin_collections;
use crate::dialogs::loading::dialog_load_mame_info;
use crate::guiutils::menuing::accel;
use crate::guiutils::menuing::iterate_menu_items;
use crate::guiutils::menuing::setup_window_menu_bar;
use crate::guiutils::menuing::show_popup_menu;
use crate::info::InfoDb;
use crate::models::collectionstree::CollectionsTreeModel;
use crate::models::itemstable::ItemsTableModel;
use crate::prefs::Preferences;
use crate::threadlocalbubble::ThreadLocalBubble;
use crate::ui::AppWindow;
use crate::Result;

#[derive(Clone, Copy, Serialize, Deserialize)]
enum MenuId {
	FileExit,
	OptionsBuiltinCollections,
	HelpRefreshInfoDb,
	HelpWebSite,
}

const MENU_PREFIX: &str = "MENU_";

impl From<MenuId> for muda::MenuId {
	fn from(value: MenuId) -> Self {
		format!("{}{}", MENU_PREFIX, serde_json::to_string(&value).unwrap()).into()
	}
}

impl TryFrom<&muda::MenuId> for MenuId {
	type Error = ();

	fn try_from(value: &muda::MenuId) -> std::result::Result<Self, Self::Error> {
		let value = value.as_ref();
		if let Some(value) = value.strip_prefix(MENU_PREFIX) {
			serde_json::from_str(value).map_err(|_| ())
		} else {
			Err(())
		}
	}
}

type InfoDbSubscriberCallback = Box<dyn Fn(Option<Rc<InfoDb>>, &Preferences)>;

struct AppModel {
	menu_bar: Menu,
	app_window_weak: Weak<AppWindow>,
	preferences: RefCell<Preferences>,
	info_db: RefCell<Option<Rc<InfoDb>>>,
	info_db_subscribers: RefCell<Vec<InfoDbSubscriberCallback>>,
}

impl AppModel {
	pub fn app_window(&self) -> AppWindow {
		self.app_window_weak.unwrap()
	}

	pub fn with_collections_tree_model<T>(&self, func: impl FnOnce(&CollectionsTreeModel) -> T) -> T {
		let collections_model = self.app_window().get_collections_model();
		let collections_model = collections_model
			.as_any()
			.downcast_ref::<CollectionsTreeModel>()
			.expect("with_collections_tree_model(): downcast_ref::<CollectionsTreeModel>() failed");
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

	pub fn modify_prefs(&self, func: impl FnOnce(&mut Preferences)) -> Result<()> {
		let mut prefs = self.preferences.borrow_mut();
		func(&mut prefs);
		drop(prefs);
		self.preferences.borrow().save()
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

pub fn create() -> AppWindow {
	let app_window = AppWindow::new().unwrap();

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
				&MenuItem::with_id(MenuId::FileExit, "Exit", true, accel("Ctrl+Alt+X")),
			],
		)
		.unwrap(),
		&Submenu::with_items("Options", true, &[
			&MenuItem::with_id(MenuId::OptionsBuiltinCollections, "Builtin Collections...", false, None)

		]).unwrap(),
		&Submenu::with_items("Settings", true, &[]).unwrap(),
		&Submenu::with_items(
			"Help",
			true,
			&[
				&MenuItem::with_id(MenuId::HelpRefreshInfoDb, "Refresh MAME machine info...", false, None),
				&MenuItem::with_id(MenuId::HelpWebSite, "BlechMAME web site...", true, None),
				&MenuItem::new("About...", false, None),
			],
		)
		.unwrap(),
	])
	.unwrap();

	// associate the Menu Bar with our window (looking forward to Slint having first class menuing support)
	setup_window_menu_bar(app_window.window(), &menu_bar);

	// get preferences
	let preferences = Preferences::load().unwrap_or_else(|_| Preferences::fresh());

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
	};
	let model = Rc::new(model);

	// the "Find MAME" button
	let model_clone = model.clone();
	app_window.on_find_mame_clicked(move || {
		on_find_mame_clicked(model_clone.clone());
	});

	// set up the collections view model
	let collections_tree_model = CollectionsTreeModel::new();
	let collections_tree_model = Rc::new(collections_tree_model);
	app_window.set_collections_model(ModelRc::new(collections_tree_model.clone()));

	// set up items view model
	let items_model = ItemsTableModel::new();
	let items_model_clone = items_model.clone();
	model.subscribe_to_info_db_changes(move |info_db, _| items_model_clone.info_db_changed(info_db));
	let items_model_clone = items_model.clone();
	app_window.set_items_model(ModelRc::new(items_model_clone));

	// InfoDB changes
	let collections_tree_model_clone = collections_tree_model.clone();
	model.subscribe_to_info_db_changes(move |info_db, prefs| {
		collections_tree_model_clone.update(info_db, &prefs.collections);
	});

	// bind collection selection changes to the items view model
	let app_window_weak = app_window.as_weak();
	collections_tree_model.on_selected_item_changed(move |collections_tree_model| {
		if let Some(row) = collections_tree_model.selected_row() {
			let app_window_weak = app_window_weak.clone();
			let row = row.try_into().unwrap();
			app_window_weak.unwrap().invoke_collections_bring_into_view(row);
		}

		// load items
		if let Some(items) = collections_tree_model.get_selected_items() {
			items_model.items_changed(items);
		}
	});

	// set up items filter
	let model_clone = model.clone();
	app_window.on_items_sort_ascending(move |index| {
		model_clone.with_items_table_model(move |x| x.sort_ascending(index));
	});
	let model_clone = model.clone();
	app_window.on_items_sort_descending(move |index| {
		model_clone.with_items_table_model(move |x| x.sort_descending(index));
	});
	let model_clone = model.clone();
	app_window.on_items_search_text_changed(move |text| {
		model_clone.with_items_table_model(move |x| x.search_text_changed(text));
	});

	// set up menu handler
	let collections_tree_model_clone = collections_tree_model.clone();
	setup_menu_handler(&model, move |model, menu_id| {
		match menu_id {
			MenuId::FileExit => {
				let _ = update_prefs(&model.clone());
				quit_event_loop().unwrap()
			}
			MenuId::OptionsBuiltinCollections => {
				let _ = update_prefs(&model.clone());
				let model = model.clone();
				let prefs = model.preferences.borrow().collections.clone();
				let collections_tree_model_clone = collections_tree_model_clone.clone();
				let fut = async move {
					if let Some(new_prefs) = dialog_builtin_collections(model.app_window(), prefs).await {
						model.preferences.borrow_mut().collections = new_prefs;
						collections_tree_model_clone
							.update(model.info_db.borrow().clone(), &model.preferences.borrow().collections);
					}
				};
				spawn_local(fut).unwrap();
			}
			MenuId::HelpRefreshInfoDb => {
				let model = model.clone();
				spawn_local(process_mame_listxml(model, None)).unwrap();
			}
			MenuId::HelpWebSite => {
				let _ = open::that("https://www.bletchmame.org");
			}
		};
	});

	// for when we shut down
	let model_clone = model.clone();
	app_window.window().on_close_requested(move || {
		let _ = update_prefs(&model_clone);
		CloseRequestResponse::HideWindow
	});

	// popup menus
	let app_window_weak = app_window.as_weak();
	app_window.on_items_row_pointer_event(move |_row, evt, point| {
		if evt.button == PointerEventButton::Right {
			let app_window = app_window_weak.unwrap();
			let window = app_window.window();
			let popup_menu = Menu::with_items(&[
				&MenuItem::new("Alpha", false, None),
				&MenuItem::new("Bravo", false, None),
				&MenuItem::new("Charlie", false, None),
			])
			.unwrap();
			show_popup_menu(window, &popup_menu, point);
		}
	});

	// spawn an effort to try to load MAME info from persisted data
	let model_clone = model.clone();
	spawn_local(try_load_persisted_info_db(model_clone)).unwrap();

	// initial update of the model; kick off the process of loading InfoDB and return
	update(&model);
	app_window
}

fn setup_menu_handler(model: &Rc<AppModel>, callback: impl Fn(&Rc<AppModel>, MenuId) + 'static + Clone) {
	let packet = (model.clone(), callback);
	let packet = ThreadLocalBubble::new(packet);

	MenuEvent::set_event_handler(Some(move |menu_event: MenuEvent| {
		if let Ok(menu_id) = MenuId::try_from(&menu_event.id) {
			let packet = packet.clone();
			invoke_from_event_loop(move || {
				let (model, callback) = packet.unwrap();
				callback(&model, menu_id);
			})
			.unwrap();
		}
	}));
}

async fn try_load_persisted_info_db(model: Rc<AppModel>) {
	// load MAME info from persisted data
	let info_db_result = model
		.preferences
		.borrow()
		.paths
		.mame_executable
		.as_deref()
		.map(InfoDb::load);

	if let Some(info_db_result) = info_db_result {
		// so... we did indeed try to load the InfoDb... but did we succeed?
		if let Ok(info_db) = info_db_result {
			// we did!  set it up
			model.set_info_db(Some(info_db));
			update(&model);
		} else {
			// we errored for whatever reason; kick off a process to read it
			process_mame_listxml(model, None).await;
		}
	}
}

/// loads MAME by launching `mame -listxml`
async fn process_mame_listxml(model: Rc<AppModel>, new_mame_executable: Option<String>) {
	// identify the MAME executable (which can be passed to us or in preferences)
	let mame_executable = new_mame_executable.as_ref().map(Cow::Borrowed).unwrap_or_else(|| {
		Cow::Owned(
			model
				.preferences
				.borrow()
				.paths
				.mame_executable
				.as_ref()
				.unwrap()
				.clone(),
		)
	});

	// present the loading dialog
	let Some(info_db) = dialog_load_mame_info(model.app_window().as_weak(), &mame_executable).await else {
		return; // cancelled
	};

	// we've succeeded; if appropriate, update the path
	if let Some(new_mame_executable) = new_mame_executable.as_ref() {
		let _ = model.modify_prefs(|prefs| prefs.paths.mame_executable = Some(new_mame_executable.clone()));
	}

	// save the info DB
	let _ = info_db.save(&mame_executable);

	// set the model to use the new Info DB
	model.set_info_db(Some(info_db));

	// and update all the things
	update(&model);
}

fn update(model: &AppModel) {
	// calculate properties
	let has_info_db = model.info_db.borrow().is_some();
	let has_mame_executable = model.preferences.borrow().paths.mame_executable.is_some();

	// update the Slint model
	model.app_window().set_has_info_db(has_info_db);

	// update the menu bar
	for menu_item in iterate_menu_items(&model.menu_bar) {
		match MenuId::try_from(menu_item.id()) {
			Ok(MenuId::HelpRefreshInfoDb) => menu_item.set_enabled(has_mame_executable),
			Ok(MenuId::OptionsBuiltinCollections) => menu_item.set_enabled(true),
			_ => {}
		}
	}
}

fn update_prefs(model: &AppModel) -> Result<()> {
	model.modify_prefs(|prefs| {
		// update window size
		let physical_size = model.app_window().window().size();
		let logical_size = physical_size.to_logical(model.app_window().window().scale_factor());
		prefs.window_size = Some(logical_size.into());

		// update collections related prefs
		prefs.collections = model.with_collections_tree_model(|x| x.get_prefs());
	})
}

fn on_find_mame_clicked(model: Rc<AppModel>) {
	// find MAME
	let mame_executable = FileDialog::new().add_filter("MAME Executable", &["exe"]).pick_file();
	if let Some(mame_executable) = mame_executable {
		let mame_executable = mame_executable.as_path().to_string_lossy().into_owned();
		spawn_local(process_mame_listxml(model, Some(mame_executable))).unwrap();
	}
}
