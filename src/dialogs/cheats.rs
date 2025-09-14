use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::CloseRequestResponse;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::SharedString;
use slint::ToSharedString;
use slint::VecModel;
use strum::EnumIter;
use strum::IntoEnumIterator;
use strum::IntoStaticStr;
use strum::VariantArray;
use tokio::sync::mpsc;

use crate::action::Action;
use crate::channel::Channel;
use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::runtime::command::MameCommand;
use crate::status::Cheat;
use crate::status::Status;
use crate::ui::CheatsDialog;
use crate::ui::CheatsDialogEntry;

struct CheatDialogModel {
	cheats: RefCell<Arc<[Cheat]>>,
	cheat_type_strings: Box<[SharedString]>,
	notify: ModelNotify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr, VariantArray, EnumIter)]
#[strum(serialize_all = "kebab-case")]
enum CheatType {
	Text,
	OneShot,
	OnOff,
	OneShotParameter,
	ValueParameter,
	ItemListParameter,
	Unknown,
}

pub async fn dialog_cheats(
	modal_stack: ModalStack,
	cheats: Arc<[Cheat]>,
	status_update_channel: Channel<Status>,
	invoke_command: impl Fn(Action) + Clone + 'static,
) {
	// prepare the dialog
	let modal = modal_stack.modal(|| CheatsDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// set up the close handler
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(());
		CloseRequestResponse::KeepWindowShown
	});

	// set up the "ok" button
	let tx_clone = tx.clone();
	modal.dialog().on_ok_clicked(move || {
		tx_clone.signal(());
	});

	// set up the model
	let model = CheatDialogModel::new(cheats);
	let model = ModelRc::new(model);
	modal.dialog().set_entries(model.clone());

	// set up command callbacks
	let model_clone = model.clone();
	let set_cheat_state = move |index: i32, enabled: bool, parameter_index: Option<i32>| {
		let model = CheatDialogModel::get_model(&model_clone);
		let index = usize::try_from(index).unwrap();
		let command = {
			let cheats = model.cheats.borrow();
			let entry = &cheats[index];
			let cheat_id = &entry.id;
			let parameter = parameter_index.map(|parameter_index| {
				let parameter_index = usize::try_from(parameter_index).unwrap();
				let parameter = entry.parameter.as_ref().unwrap();
				parameter.items[parameter_index].value
			});
			MameCommand::set_cheat_state(cheat_id, enabled, parameter).into()
		};
		invoke_command(command);
	};
	let set_cheat_state = Rc::new(set_cheat_state) as Rc<dyn Fn(i32, bool, Option<i32>)>;
	let set_cheat_state_clone = set_cheat_state.clone();
	modal.dialog().on_set_cheat_state(move |index, enabled| {
		set_cheat_state_clone(index, enabled, None);
	});
	modal
		.dialog()
		.on_set_cheat_state_parameter(move |index, enabled, parameter_index| {
			set_cheat_state(index, enabled, Some(parameter_index));
		});

	// subscribe to status changes
	let _subscription = status_update_channel.subscribe(move |status| {
		let model = CheatDialogModel::get_model(&model);
		let empty_cheats = Arc::default();
		let cheats = status.running.as_ref().map(|r| &r.cheats).unwrap_or(&empty_cheats);
		model.update(cheats);
	});

	// present the modal dialog
	modal.run(async { rx.recv().await.unwrap() }).await;
}

impl CheatDialogModel {
	pub fn new(cheats: Arc<[Cheat]>) -> Self {
		let cheats = RefCell::new(cheats);
		let cheat_type_strings = CheatType::iter()
			.map(|ct| {
				let ct: &'static str = ct.into();
				ct.into()
			})
			.collect();
		Self {
			cheats,
			cheat_type_strings,
			notify: ModelNotify::default(),
		}
	}

	pub fn update(&self, cheats: &Arc<[Cheat]>) {
		if self.cheats.borrow().as_ref() != cheats.as_ref() {
			self.cheats.replace(cheats.clone());
			self.notify.reset();
		}
	}

	pub fn get_model(model: &ModelRc<CheatsDialogEntry>) -> &Self {
		model.as_any().downcast_ref::<Self>().unwrap()
	}
}

impl Model for CheatDialogModel {
	type Data = CheatsDialogEntry;

	fn row_count(&self) -> usize {
		self.cheats.borrow().len()
	}

	fn row_data(&self, index: usize) -> Option<Self::Data> {
		// get the basics
		let cheats = self.cheats.borrow();
		let entry = cheats.get(index)?;
		let parameter = entry.parameter.as_ref();

		// determine the cheat type string
		let cheat_type = classify_cheat(entry);
		let cheat_type_index = CheatType::VARIANTS.iter().position(|p| *p == cheat_type).unwrap();
		let cheat_type = self.cheat_type_strings[cheat_type_index].clone();

		// miscellaneous
		let cheat_enabled = entry.enabled;
		let description = entry.description.to_shared_string();
		let comment = entry.comment.to_shared_string();
		let has_changed_script = entry.has_changed_script;

		// numeric values
		let value = parameter
			.map(|p| p.value.parse().unwrap_or_default())
			.unwrap_or_default();
		let minimum = parameter.map(|p| p.minimum.try_into().unwrap()).unwrap_or_default();
		let maximum = parameter.map(|p| p.maximum.try_into().unwrap()).unwrap_or_default();
		let step = parameter.map(|p| p.step.try_into().unwrap()).unwrap_or_default();

		// and items
		let items = parameter
			.map(|p| p.items.as_slice())
			.unwrap_or_default()
			.iter()
			.map(|item| item.text.to_shared_string())
			.collect::<Vec<_>>();
		let items = VecModel::from(items);
		let items = ModelRc::new(items);

		// and return the entry
		let entry = Self::Data {
			cheat_type,
			cheat_enabled,
			description,
			comment,
			has_changed_script,
			value,
			minimum,
			maximum,
			step,
			items,
		};
		Some(entry)
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

fn classify_cheat(cheat: &Cheat) -> CheatType {
	match (
		cheat.has_on_script,
		cheat.has_off_script,
		cheat.has_run_script,
		cheat.has_changed_script,
		cheat.parameter.as_ref().map(|p| p.items.is_empty()),
	) {
		(false, false, false, _, None) => CheatType::Text,
		(true, false, false, _, None) => CheatType::OneShot,
		(true, true, _, _, _) => CheatType::OnOff,
		(_, _, true, _, None) => CheatType::OnOff,
		(false, false, _, true, Some(false)) => CheatType::OneShotParameter,
		(_, _, _, _, Some(true)) => CheatType::ValueParameter,
		(_, _, _, _, Some(false)) => CheatType::ItemListParameter,
		_ => CheatType::Unknown,
	}
}
