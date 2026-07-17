//! Public transfer-model constructor coverage.

use pcloud_model::{
    ids::SyncId,
    sync::PlannedOperation,
    transfer::{TransferState, TransferTask},
};

#[test]
fn planned_transfer_starts_without_an_error_and_round_trips() {
    let task = TransferTask::planned(PlannedOperation::DeleteLocal {
        sync_id: SyncId::new(7),
        path: "coverage.txt".to_owned(),
    });
    assert_eq!(task.state, TransferState::Planned);
    assert!(task.last_error.is_none());
    let encoded = serde_json::to_string(&task).unwrap();
    assert_eq!(
        serde_json::from_str::<TransferTask>(&encoded).unwrap(),
        task
    );
}
