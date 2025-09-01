use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use itertools::Itertools;
use slint::CloseRequestResponse;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::ToSharedString;
use slint::VecModel;
use tokio::sync::mpsc;

use crate::appcommand::AppCommand;
use crate::channel::Channel;
use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::info::ConditionRelation;
use crate::info::InfoDb;
use crate::info::View;
use crate::runtime::command::MameCommand;
use crate::status::Input;
use crate::status::InputClass;
use crate::status::Status;
use crate::ui::SwitchesDialog;
use crate::ui::SwitchesDialogEntry;

struct SwitchesDialogModel {
	state: RefCell<SwitchesDialogState>,
	class: InputClass,
	info_db: Rc<InfoDb>,
	notify: ModelNotify,
}

#[derive(Debug, Default)]
struct SwitchesDialogState {
	pub machine_index: Option<usize>,
	pub inputs: Arc<[Input]>,
	pub entries: Box<[Entry]>,
}

#[derive(Debug)]
struct Entry {
	pub input_index: usize,
	pub config: Option<EntryConfig>,
}

#[derive(Debug)]
struct EntryConfig {
	pub config_index: usize,
	pub option_indexes: Box<[usize]>,
}

pub async fn dialog_switches(
	modal_stack: ModalStack,
	inputs: Arc<[Input]>,
	info_db: Rc<InfoDb>,
	class: InputClass,
	machine_index: Option<usize>,
	status_update_channel: Channel<Status>,
	invoke_command: impl Fn(AppCommand) + Clone + 'static,
) {
	// prepare the dialog
	let modal = modal_stack.modal(|| SwitchesDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// set the title
	modal.dialog().set_dialog_title(class.title().into());

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
	let model = SwitchesDialogModel::new(class, info_db.clone());
	let model = Rc::new(model);
	let model = ModelRc::new(model);
	modal.dialog().set_entries(model.clone());

	// set up the callback for when an option changes
	let model_clone = model.clone();
	modal
		.dialog()
		.on_entry_option_changed(move |setting_index, option_index| {
			let model = SwitchesDialogModel::get_model(&model_clone);
			let setting_index = setting_index.try_into().unwrap();
			let option_index = option_index.try_into().unwrap();
			let command = model.set_setting_option_command(setting_index, option_index);
			invoke_command(command);
		});

	// subscribe to status changes
	let model_clone = model.clone();
	let _subscription = status_update_channel.subscribe(move |status| {
		// update the model
		let model = SwitchesDialogModel::get_model(&model_clone);
		let running = status.running.as_ref();
		let machine_index = running.and_then(|r| info_db.machines().find_index(r.machine_name.as_str()).ok());
		let inputs = running.map(|r| &r.inputs).cloned().unwrap_or_default();
		model.update(machine_index, inputs);
	});

	// update the model
	SwitchesDialogModel::get_model(&model).update(machine_index, inputs);

	// present the modal dialog
	modal.run(async { rx.recv().await.unwrap() }).await;
}

impl SwitchesDialogModel {
	pub fn new(class: InputClass, info_db: Rc<InfoDb>) -> Self {
		let state = SwitchesDialogState::default();
		let state = RefCell::new(state);
		let notify = ModelNotify::default();
		Self {
			state,
			class,
			info_db,
			notify,
		}
	}

	pub fn update(&self, machine_index: Option<usize>, inputs: Arc<[Input]>) {
		let changed = {
			let mut state = self.state.borrow_mut();
			let changed = state.machine_index != machine_index || state.inputs != inputs;
			if changed {
				state.machine_index = machine_index;
				state.inputs = inputs;
				state.entries = build_entries(&self.info_db, self.class, machine_index, &state.inputs);
			}
			changed
		};
		if changed {
			self.notify.reset();
		}
	}

	pub fn set_setting_option_command(&self, setting_index: usize, option_index: usize) -> AppCommand {
		let state = self.state.borrow();
		let entry = &state.entries[setting_index];
		let input = &state.inputs[entry.input_index];
		let config = entry.config.as_ref().unwrap();
		let option_index = config.option_indexes[option_index];

		let machine = self.info_db.machines().get(state.machine_index.unwrap()).unwrap();
		let infodb_config = machine.configurations().get(config.config_index).unwrap();

		let port_tag = &input.port_tag;
		let mask = input.mask;
		let value = infodb_config.settings().get(option_index).unwrap().value();

		MameCommand::set_input_value(port_tag, mask, value).into()
	}

	pub fn get_model(model: &impl Model) -> &'_ Self {
		model.as_any().downcast_ref::<Self>().unwrap()
	}
}

impl Model for SwitchesDialogModel {
	type Data = SwitchesDialogEntry;

	fn row_count(&self) -> usize {
		self.state.borrow().entries.len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		let state = self.state.borrow();
		let entry = state.entries.get(row)?;
		let input = state.inputs.get(entry.input_index).unwrap();
		let name = input.name.to_shared_string();

		let machine_index = state.machine_index?;
		let machine = self.info_db.machines().get(machine_index).unwrap();

		// build the options list
		let (options, current_option_index) = entry
			.config
			.as_ref()
			.map(|config| {
				let settings = machine.configurations().get(config.config_index).unwrap().settings();
				let options = config
					.option_indexes
					.iter()
					.map(|&setting_index| settings.get(setting_index).unwrap().name().to_shared_string())
					.collect::<Vec<_>>();
				let current_options_index = settings.iter().position(|s| input.value == Some(s.value()));
				(options, current_options_index)
			})
			.unwrap_or((Vec::new(), None));
		let options = VecModel::from(options);
		let options = ModelRc::new(options);
		let current_option_index = current_option_index.map(|x| x.try_into().unwrap()).unwrap_or(-1);

		let entry = SwitchesDialogEntry {
			name,
			options,
			current_option_index,
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

fn build_entries(info_db: &InfoDb, class: InputClass, machine_index: Option<usize>, inputs: &[Input]) -> Box<[Entry]> {
	let Some(machine_index) = machine_index else {
		return Box::new([]);
	};

	let configurations = info_db.machines().get(machine_index).unwrap().configurations();

	inputs
		.iter()
		.enumerate()
		.filter(|(_, input)| input.class == Some(class))
		.map(|(input_index, input)| {
			let config_and_index = configurations
				.iter()
				.find_position(|c| c.tag() == input.port_tag && c.mask() == input.mask);

			let config = config_and_index.map(|(config_index, config)| {
				let option_indexes = config
					.settings()
					.iter()
					.enumerate()
					.filter(|(_, setting)| {
						setting.conditions().iter().all(|cond| {
							evaluate_condition(inputs, cond.tag(), cond.relation(), cond.mask(), cond.value())
						})
					})
					.map(|(setting_index, _)| setting_index)
					.collect();
				EntryConfig {
					config_index,
					option_indexes,
				}
			});

			Entry { input_index, config }
		})
		.collect()
}

fn evaluate_condition(inputs: &[Input], tag: &str, relation: ConditionRelation, mask: u32, value: u32) -> bool {
	let input = inputs.iter().find(|x| x.port_tag.as_str() == tag).unwrap();
	let Some(input_value) = input.value else {
		return false;
	};
	let input_value = input_value & mask;
	match relation {
		ConditionRelation::Eq => input_value == value,
		ConditionRelation::Ne => input_value != value,
		ConditionRelation::Gt => input_value > value,
		ConditionRelation::Le => input_value <= value,
		ConditionRelation::Lt => input_value < value,
		ConditionRelation::Ge => input_value >= value,
	}
}
