use std::any::Any;
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

use crate::appcommand::AppCommand;
use crate::channel::Channel;
use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::status::Cheat;
use crate::status::Status;
use crate::ui::CheatsDialog;
use crate::ui::CheatsDialogEntry;

struct CheatDialogModel {
	cheats: Arc<[Cheat]>,
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
	_status_update_channel: Channel<Status>,
	_invoke_command: impl Fn(AppCommand) + Clone + 'static,
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
	modal.dialog().set_entries(model);

	// present the modal dialog
	modal.run(async { rx.recv().await.unwrap() }).await;
}

impl CheatDialogModel {
	pub fn new(cheats: Arc<[Cheat]>) -> Self {
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
}

impl Model for CheatDialogModel {
	type Data = CheatsDialogEntry;

	fn row_count(&self) -> usize {
		self.cheats.len()
	}

	fn row_data(&self, index: usize) -> Option<Self::Data> {
		// get the basics
		let entry = self.cheats.get(index)?;
		let parameter = entry.parameter.as_ref();

		// determine the cheat type string
		let cheat_type = classify_cheat(entry);
		let cheat_type_index = CheatType::VARIANTS.iter().position(|p| *p == cheat_type).unwrap();
		let cheat_type = self.cheat_type_strings[cheat_type_index].clone();

		// is the cheat enabled/activated?
		let cheat_enabled = entry.enabled;

		// text stuff
		let description = entry.description.to_shared_string();
		let comment = entry.comment.to_shared_string();

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
