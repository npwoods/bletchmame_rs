use std::borrow::Cow;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use itertools::Itertools;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::Model;
use slint::ModelRc;
use slint::VecModel;
use tokio::sync::Notify;

use crate::guiutils::windowing::with_modal_parent;
use crate::prefs::BuiltinCollectionItem;
use crate::prefs::InnerCollectionItem;
use crate::prefs::PrefsCollectionItem;
use crate::ui::BuiltinCollectionDisplay;
use crate::ui::BuiltinCollectionsDialog;

struct Entry<'a> {
	item: BuiltinCollectionItem,
	usage: Vec<Vec<&'a str>>,
}

pub async fn dialog_builtin_collections(
	parent: (impl ComponentHandle + 'static),
	prefs: Vec<PrefsCollectionItem>,
) -> Option<Vec<PrefsCollectionItem>> {
	// the first step is to create a list of entries for all builtins
	let entries = {
		// create an entry for each build type
		let mut entries = BuiltinCollectionItem::all_values()
			.iter()
			.map(|&item| Entry {
				item,
				usage: Vec::new(),
			})
			.collect::<Vec<_>>();

		// and identify all usage
		let mut current_path = Vec::new();
		PrefsCollectionItem::walk(&prefs, |item, path| {
			current_path.resize(path.len(), Default::default());
			match &item.inner {
				InnerCollectionItem::Builtin(item) => {
					let entry = entries.iter_mut().find(|x| x.item == *item).unwrap();
					entry.usage.push(current_path.clone());
				}
				InnerCollectionItem::Folder(item) => {
					current_path.push(item.name.as_str());
				}
				_ => {}
			}
		});

		// and return
		entries.shrink_to_fit();
		entries
	};

	let displays = entries
		.iter()
		.map(|entry| BuiltinCollectionDisplay {
			checked: !entry.usage.is_empty(),
			original_checked: !entry.usage.is_empty(),
			comments: usage_string(&entry.usage).into(),
			text: format!("{}", entry.item).into(),
		})
		.collect::<Vec<_>>();

	// create the model to attach to the dialog
	let model = VecModel::from(displays);
	let model = Rc::from(model);

	// prepare the dialog
	let notify = Arc::new(Notify::new());
	let dialog = with_modal_parent(&parent, || BuiltinCollectionsDialog::new().unwrap());
	dialog.set_model(ModelRc::new(model.clone()));

	// set callbacks for when we're done
	let notify_clone = notify.clone();
	dialog.on_done(move || notify_clone.notify_one());
	let notify_clone = notify.clone();
	dialog.window().on_close_requested(move || {
		notify_clone.notify_one();
		CloseRequestResponse::HideWindow
	});

	// set up a callback to properly enable/disable the ok button
	let model_clone = model.clone();
	let dialog_weak = dialog.as_weak();
	dialog.on_update_ok_enabled(move || {
		let ok_enabled = model_clone
			.iter()
			.any(|display| display.checked != display.original_checked);
		dialog_weak.unwrap().set_ok_enabled(ok_enabled);
	});

	dialog.show().unwrap();

	// wait for completion
	notify.notified().await;
	dialog.hide().unwrap();

	// did we accept
	dialog
		.get_accepted()
		.then(move || {
			#[derive(Copy, Clone, Debug, PartialEq, Eq)]
			enum ChangeType {
				Addition,
				Subtraction,
			}

			// identify the change set that needs to be made
			let changes = Iterator::zip(model.iter(), BuiltinCollectionItem::all_values().iter())
				.filter_map(|(display, item)| match (display.original_checked, display.checked) {
					(false, true) => Some((*item, ChangeType::Addition)),
					(true, false) => Some((*item, ChangeType::Subtraction)),
					_ => None,
				})
				.collect::<Vec<_>>();

			// do we actually have any changes?
			(!changes.is_empty()).then(|| {
				// we do - first apply the removals
				let removals = changes
					.iter()
					.filter_map(|(item, change_type)| (*change_type == ChangeType::Subtraction).then_some(*item))
					.collect::<HashSet<_>>();
				let mut prefs = PrefsCollectionItem::process(prefs, |items| {
					items
						.into_iter()
						.filter(|item| !matches!(item.inner, InnerCollectionItem::Builtin(x) if removals.contains(&x)))
						.collect()
				});

				// then apply the additions
				prefs.extend(changes.iter().filter_map(|(item, change_type)| {
					(*change_type == ChangeType::Addition).then_some(PrefsCollectionItem {
						selected: Default::default(),
						inner: InnerCollectionItem::Builtin(*item),
					})
				}));

				// and finally return
				prefs
			})
		})
		.flatten()
}

fn usage_string(usage: &[Vec<&str>]) -> String {
	usage
		.iter()
		.flat_map(|x| x.iter())
		.any(|x| !x.is_empty())
		.then(|| {
			let result = usage
				.iter()
				.map(|x| {
					if x.is_empty() {
						Cow::from("<top>")
					} else {
						Cow::from(x.iter().join(" / "))
					}
				})
				.join(", ");
			format!("({})", result)
		})
		.unwrap_or_default()
}
