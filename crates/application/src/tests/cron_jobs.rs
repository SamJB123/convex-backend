use std::{
    collections::BTreeMap,
    str::FromStr,
    time::Duration,
};

use common::{
    document::ParsedDocument,
    query::{
        IndexRange,
        IndexRangeExpression,
        Order,
        Query,
    },
    runtime::Runtime,
};
use database::{
    query::TableFilter,
    DeveloperQuery,
    TableModel,
    Transaction,
};
use isolate::parse_udf_args;
use keybroker::Identity;
use model::{
    backend_state::{
        types::BackendState,
        BackendStateModel,
    },
    cron_jobs::{
        types::{
            CronIdentifier,
            CronJob,
            CronSchedule,
            CronSpec,
        },
        CronModel,
        CRON_JOB_LOGS_INDEX_BY_NAME_TS,
        CRON_JOB_LOGS_NAME_FIELD,
    },
};
use runtime::testing::TestRuntime;
use serde_json::Value as JsonValue;
use sync_types::UdfPath;

use crate::{
    test_helpers::{
        ApplicationTestExt,
        OBJECTS_TABLE,
    },
    Application,
};

fn test_cron_identifier() -> CronIdentifier {
    CronIdentifier::from_str("test").unwrap()
}

async fn create_cron_job(
    tx: &mut Transaction<TestRuntime>,
) -> anyhow::Result<(
    BTreeMap<CronIdentifier, ParsedDocument<CronJob>>,
    CronModel<TestRuntime>,
)> {
    let mut cron_model = CronModel::new(tx);
    let mut map = serde_json::Map::new();
    map.insert(
        "key".to_string(),
        serde_json::Value::String("value".to_string()),
    );
    let udf_path = UdfPath::from_str("basic:insertObject").unwrap();
    let cron_spec = CronSpec {
        udf_path: udf_path.clone().canonicalize(),
        udf_args: parse_udf_args(&udf_path, vec![JsonValue::Object(map)])?,
        cron_schedule: CronSchedule::Interval { seconds: 60 },
    };
    let original_jobs = cron_model.list().await?;
    let name = test_cron_identifier();
    cron_model.create(name, cron_spec).await?;
    Ok((original_jobs, cron_model))
}

fn cron_log_query<RT: Runtime>(tx: &mut Transaction<RT>) -> anyhow::Result<DeveloperQuery<RT>> {
    DeveloperQuery::new(
        tx,
        Query::index_range(IndexRange {
            index_name: CRON_JOB_LOGS_INDEX_BY_NAME_TS.clone(),
            range: vec![IndexRangeExpression::Eq(
                CRON_JOB_LOGS_NAME_FIELD.clone(),
                common::types::MaybeValue(Some(test_cron_identifier().to_string().try_into()?)),
            )],
            order: Order::Asc,
        }),
        TableFilter::IncludePrivateSystemTables,
    )
}

#[ignore] // TODO(CX-6058) Fix this test and remove the ignore
#[convex_macro::test_runtime]
pub(crate) async fn test_cron_jobs_success(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    application.load_udf_tests_modules().await?;

    let mut tx = application.begin(Identity::system()).await?;

    let (original_jobs, mut cron_model) = create_cron_job(&mut tx).await?;

    let jobs = cron_model.list().await?;
    assert_eq!(jobs.len(), original_jobs.len() + 1);

    let mut table_model = TableModel::new(&mut tx);
    assert!(table_model.table_is_empty(&OBJECTS_TABLE).await?);

    application.commit_test(tx).await?;

    // Cron jobs executor within application will pick up the job and
    // execute it. Add some wait time to make this less racy.
    rt.wait(Duration::from_secs(1)).await;
    let mut tx = application.begin(Identity::system()).await?;
    let mut table_model = TableModel::new(&mut tx);
    assert!(!table_model.table_is_empty(&OBJECTS_TABLE).await?);
    let mut logs_query = cron_log_query(&mut tx)?;
    logs_query.expect_one(&mut tx).await?;
    Ok(())
}

#[convex_macro::test_runtime]
pub(crate) async fn test_cron_jobs_race_condition(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    application.load_udf_tests_modules().await?;

    let mut tx = application.begin(Identity::system()).await?;
    let (original_jobs, mut model) = create_cron_job(&mut tx).await?;

    let jobs = model.list().await?;
    assert_eq!(jobs.len(), original_jobs.len() + 1);
    let job_doc = jobs.get(&test_cron_identifier()).unwrap();
    let (job_id, job) = job_doc.clone().into_id_and_value();

    // Delete the cron job
    model.delete(job_doc.clone()).await?;
    let jobs = model.list().await?;
    assert_eq!(jobs.len(), original_jobs.len());

    application.commit_test(tx).await?;

    // This simulates the race condition where the job executor picks up a cron
    // to execute after the cron was created but before it was deleted. We should
    // handle the race condition gracefully instead of trying to run the stale cron.
    application
        .test_one_off_cron_job_executor_run(job, job_id)
        .await?;
    Ok(())
}

#[ignore] // TODO(CX-6058) Fix this test and remove the ignore
#[convex_macro::test_runtime]
async fn test_paused_cron_jobs(rt: TestRuntime) -> anyhow::Result<()> {
    test_cron_jobs_helper(rt, BackendState::Paused).await?;

    Ok(())
}

#[ignore] // TODO(CX-6058) Fix this test and remove the ignore
#[convex_macro::test_runtime]
async fn test_disable_cron_jobs(rt: TestRuntime) -> anyhow::Result<()> {
    test_cron_jobs_helper(rt, BackendState::Disabled).await?;

    Ok(())
}

async fn test_cron_jobs_helper(rt: TestRuntime, backend_state: BackendState) -> anyhow::Result<()> {
    // Helper for testing behavior for pausing or disabling backends
    let application = Application::new_for_tests(&rt).await?;
    application.load_udf_tests_modules().await?;

    // Change backend state
    let mut tx = application.begin(Identity::system()).await?;
    let mut backend_state_model = BackendStateModel::new(&mut tx);
    backend_state_model
        .toggle_backend_state(backend_state)
        .await?;
    application.commit_test(tx).await?;

    let mut tx = application.begin(Identity::system()).await?;
    let (original_jobs, mut cron_model) = create_cron_job(&mut tx).await?;
    let jobs = cron_model.list().await?;
    assert_eq!(jobs.len(), original_jobs.len() + 1);
    let mut table_model = TableModel::new(&mut tx);
    assert!(table_model.table_is_empty(&OBJECTS_TABLE).await?);
    application.commit_test(tx).await?;

    // Cron jobs executor within application will pick up the job and
    // execute it. Add some wait time to make this less racy. Job should not execute
    // because the backend is paused.
    rt.wait(Duration::from_secs(1)).await;
    let mut tx = application.begin(Identity::system()).await?;
    let mut table_model = TableModel::new(&mut tx);
    assert!(table_model.table_is_empty(&OBJECTS_TABLE).await?);
    let mut logs_query = cron_log_query(&mut tx)?;
    logs_query.expect_none(&mut tx).await?;

    // Resuming the backend should make the jobs execute.
    let mut model = BackendStateModel::new(&mut tx);
    model.toggle_backend_state(BackendState::Running).await?;
    application.commit_test(tx).await?;
    rt.wait(Duration::from_secs(1)).await;
    let mut tx = application.begin(Identity::system()).await?;
    let mut table_model = TableModel::new(&mut tx);
    assert!(!table_model.table_is_empty(&OBJECTS_TABLE).await?);
    let mut logs_query = cron_log_query(&mut tx)?;
    logs_query.expect_one(&mut tx).await?;

    Ok(())
}
