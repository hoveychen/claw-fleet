mod common;

use axum::http::StatusCode;

#[sqlx::test(migrations = "./migrations")]
async fn missing_key_is_401(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    assert_eq!(
        common::get_tasks(common::router(pool), None).await,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn unknown_key_is_401(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    assert_eq!(
        common::get_tasks(
            common::router(pool),
            Some("flk_test_unknown_0123456789abcdef")
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn revoked_key_is_401(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    assert_eq!(
        common::get_tasks(common::router(pool), Some(common::REVOKED_KEY)).await,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn valid_project_key_is_200(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    assert_eq!(
        common::get_tasks(common::router(pool), Some(common::VALID_KEY)).await,
        StatusCode::OK
    );
}
