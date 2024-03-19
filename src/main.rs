mod handlers;
mod repositories;

use crate::repositories::label::{LabelRepositoryForDb, LabelRepository};
use crate::repositories::todo::{
    TodoRepository, 
    TodoRepositoryForDb
};

use axum::{
    extract::Extension,
    http::HeaderValue,
    routing::{get, post, delete},
    Router,
};
use dotenv::dotenv;
use handlers::{
    label::{create_label, all_label, delete_label},
    todo::{all_todo, create_todo, delete_todo, find_todo, update_todo},
};
use hyper::header::CONTENT_TYPE;
use sqlx::PgPool;
use std::{env, sync::Arc};
use tower_http::cors::{Any, CorsLayer};

#[tokio::main]
async fn main() {
    // loggingの初期化
    // let log_level = env::var("RUST_LOG").unwrap_or("info".to_string());
    // env::set_var("RUST_LOG", log_level);
    tracing_subscriber::fmt::init();
    dotenv().ok();

    let database_url = &env::var("DATABASE_URL").expect("undefined [DATABASE_URL]");
    tracing::debug!("start connect database...");
    let pool = PgPool::connect(database_url)
        .await
        .unwrap_or_else(|_| panic!("fail connect database, url is [{}]", database_url));
    let app = create_app(
        TodoRepositoryForDb::new(pool.clone()),
        LabelRepositoryForDb::new(pool.clone()));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn create_app<Todo: TodoRepository, Label: LabelRepository>(todo_repository: Todo, label_repository: Label) -> Router {
    Router::new()
        .route("/", get(root))
        .route(
            "/todos",
            post(create_todo::<Todo>)
            .get(all_todo::<Todo>)
        )
        .route(
            "/todos/:id",
            get(find_todo::<Todo>)
            .delete(delete_todo::<Todo>)
            .patch(update_todo::<Todo>),
        )
        .route(
            "/labels",
            post(create_label::<Label>)
            .get(all_label::<Label>)
        )
        .route("/labels/:id", delete(delete_label::<Label>))

        .layer(Extension(Arc::new(todo_repository)))
        .layer(Extension(Arc::new(label_repository)))
        .layer(
            CorsLayer::new()
                .allow_origin("http://localhost:3001".parse::<HeaderValue>().unwrap())
                .allow_methods(Any)
                .allow_headers(vec![CONTENT_TYPE]),
        )
}

async fn root() -> &'static str {
    "Hello World!"
}

#[cfg(test)]
mod test {
    use self::repositories::label::Label;

    use super::*;
    use crate::repositories::label::test_utils::LabelRepositoryForMemory;
    use crate::repositories::todo::{
        test_utils::TodoRepositoryForMemory,
        CreateTodo,
        TodoEntity,
    };
    
    use axum::response::Response;
    use axum::{
        body,
        body::Body,
        http::{header, Method, Request, StatusCode},
    };
    use tower::ServiceExt;

    fn build_todo_req_with_json(path: &str, method: Method, json_body: String) -> Request<Body> {
        Request::builder()
            .uri(path)
            .method(method)
            .header(header::CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
            .body(Body::from(json_body))
            .unwrap()
    }

    fn build_todo_req_with_empty(method: Method, path: &str) -> Request<Body> {
        Request::builder()
            .uri(path)
            .method(method)
            .body(Body::empty())
            .unwrap()
    }

    async fn res_to_todo(res: Response) -> TodoEntity {
        let bytes = body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        let todo = serde_json::from_str(&body)
            .expect(&format!("cannot convert Todo instance. body: {}", body));
        todo
    }

    async fn res_to_label(res: Response) -> Label {
        let bytes = body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        let label = serde_json::from_str(&body)
            .expect(&format!("cannot convert Label instance. body: {}", body));
        label
    }

    fn labels_values_tuple() -> (Vec<i32>, Vec<Label>){
        let id = 1;
        (
            vec![id],
            vec![
                Label {
                    id,
                    name: String::from("label test 1"),
                }
            ],
        )
    }

    #[tokio::test]
    async fn should_created_todo() {
        let (_, labels) = labels_values_tuple();
        let expected = TodoEntity::new(1, "should_return_created_todo".to_string(), labels.clone());
        let todo_repository = TodoRepositoryForMemory::new(labels);
        let label_repository = LabelRepositoryForMemory::new();
        let req = build_todo_req_with_json(
            "/todos",
            Method::POST,
            r#"{ "text": "should_return_created_todo", "labels": [1] }"#.to_string(),
        );
        let res = create_app(todo_repository, label_repository).oneshot(req).await.unwrap();
        let todo = res_to_todo(res).await;
        assert_eq!(expected, todo);
    }

    // #[tokio::test]
    // async fn should_return_hello_world() {
    //     let repository = TodoRepositoryForMemory::new();
    //     let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    //     let res = create_app(repository).oneshot(req).await.unwrap();
    //     let bytes = body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    //     let body = String::from_utf8(bytes.to_vec()).unwrap();
    //     assert_eq!(body, "Hello World!");
    // }

    #[tokio::test]
    async fn should_find_todo() {
        let (label_id, labels) = labels_values_tuple();
        let expected = TodoEntity::new(1, "should_find_todo".to_string(), labels.clone());
        let todo_repository = TodoRepositoryForMemory::new(labels.clone());
        let label_repository = LabelRepositoryForMemory::new();
        todo_repository
            .create(CreateTodo::new("should_find_todo".to_string(), label_id))
            .await
            .expect("failed create todo");
        let req = build_todo_req_with_empty(Method::GET, "/todos/1");
        let res = create_app(todo_repository, label_repository).oneshot(req).await.unwrap();
        let todo = res_to_todo(res).await;
        assert_eq!(expected, todo);
    }

    #[tokio::test]
    async fn should_get_all_todos() {
        let (label_id, labels) = labels_values_tuple();
        let expected = TodoEntity::new(1, "should_get_all_todos".to_string(), labels.clone());
        let todo_repository = TodoRepositoryForMemory::new(labels.clone());
        let label_repository = LabelRepositoryForMemory::new();
        todo_repository
            .create(CreateTodo::new("should_get_all_todos".to_string(), label_id))
            .await
            .expect("failed create todo");
        let req = build_todo_req_with_empty(Method::GET, "/todos");
        let res = create_app(todo_repository, label_repository).oneshot(req).await.unwrap();
        let bytes = body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        let todo: Vec<TodoEntity> = serde_json::from_str(&body)
            .expect(&format!("cannot convert Todo instance. body: {}", body));
        assert_eq!(vec![expected], todo);
    }

    #[tokio::test]
    async fn should_update_todo() {
        let (label_id, labels) = labels_values_tuple();
        let expected = TodoEntity::new(1, "should_update_todo".to_string(), labels.clone());
        let todo_repository = TodoRepositoryForMemory::new(labels.clone());
        let label_repository = LabelRepositoryForMemory::new();
        todo_repository
            .create(CreateTodo::new("should_update_todo".to_string(), label_id))
            .await
            .expect("failed create todo");
        let req = build_todo_req_with_json(
            "/todos/1",
            Method::PATCH,
            r#"{"id": 1, "text": "should_update_todo", "completed": false}"#.to_string(),
        );
        let res = create_app(todo_repository, label_repository).oneshot(req).await.unwrap();
        let todo = res_to_todo(res).await;
        assert_eq!(expected, todo);
    }

    #[tokio::test]
    async fn should_delete_todo() {
        let (label_id, labels) = labels_values_tuple();
        let todo_repository = TodoRepositoryForMemory::new(labels.clone());
        let label_repository = LabelRepositoryForMemory::new();
        todo_repository
            .create(CreateTodo::new("should_delete_todo".to_string(),label_id))
            .await
            .expect("failed delete todo");
        let req = build_todo_req_with_empty(Method::DELETE, "/todos/1");
        let res = create_app(todo_repository, label_repository).oneshot(req).await.unwrap();
        assert_eq!(StatusCode::NO_CONTENT, res.status());
    }

    #[tokio::test]
    async fn should_created_label() {
        let expected = Label::new(1, "should_create_label".to_string());
        let todo_repository = TodoRepositoryForMemory::new(vec![expected.clone()]);
        let label_repository = LabelRepositoryForMemory::new();
        let req = build_todo_req_with_json(
            "/labels",
            Method::POST,
            r#"{ "name": "should_create_label" }"#.to_string()
        );
        let res = create_app(todo_repository, label_repository).oneshot(req).await.unwrap();
        let label = res_to_label(res).await;
        assert_eq!(expected, label);    
    }

    #[tokio::test]
    async fn should_all_label() {
        let expected = Label::new(1, "should_create_label".to_string());
        let todo_repository = TodoRepositoryForMemory::new(vec![expected.clone()]);
        let label_repository = LabelRepositoryForMemory::new();
        label_repository
            .create("should_create_label".to_string())
            .await
            .expect("failed all label");
        let req = build_todo_req_with_empty(Method::GET, "/labels");
        let res = create_app(todo_repository, label_repository).oneshot(req).await.unwrap();
        let bytes = body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        let label: Vec<Label> = serde_json::from_str(&body)
            .expect(&format!("cannot convert Label instance. body: {}", body));
        assert_eq!(vec![expected], label);
    }

    #[tokio::test]
    async fn should_delete_label() {
        let label = Label::new(1, "should_create_label".to_string());
        let todo_repository = TodoRepositoryForMemory::new(vec![label.clone()]);
        let label_repository = LabelRepositoryForMemory::new();
        label_repository
            .create("should_create_label".to_string())
            .await
            .expect("failed all label");
        let req = build_todo_req_with_empty(Method::DELETE, "/labels/1");
        let res = create_app(todo_repository, label_repository).oneshot(req).await.unwrap();
        assert_eq!(StatusCode::NO_CONTENT, res.status());
    }
}
