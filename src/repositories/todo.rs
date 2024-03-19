use anyhow::Ok;
use axum::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use validator::Validate;
use crate::repositories::RepositoryError;

use super::label::Label;


#[async_trait]
pub trait TodoRepository: Clone + std::marker::Send + std::marker::Sync + 'static {
    async fn create(&self, payload: CreateTodo) -> anyhow::Result<TodoEntity>;
    async fn find(&self, id: i32) -> anyhow::Result<TodoEntity>;
    async fn all(&self) -> anyhow::Result<Vec<TodoEntity>>;
    async fn update(&self, id: i32, payload: UpdateTodo) -> anyhow::Result<TodoEntity>;
    async fn delete(&self, id: i32) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct TodoWithLabelFromRow {
    id: i32,
    text: String,
    completed: bool,
    label_id: Option<i32>,
    label_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct TodoEntity {
    pub id: i32,
    pub text: String, 
    pub completed: bool,
    pub labels: Vec<Label>,
}

fn fold_entities(rows: Vec<TodoWithLabelFromRow>) -> Vec<TodoEntity> {
    let mut rows = rows.iter();
    let mut accum: Vec<TodoEntity> = vec![];
    'outer: while let Some(row) = rows.next() {
        let mut todos = accum.iter_mut();
        while let Some(todo) = todos.next() {
            if todo.id == row.id {
                todo.labels.push(Label {
                    id: row.label_id.unwrap(),
                    name: row.label_name.clone().unwrap(),
                });
                continue 'outer;
            }
        }

        let labels = if row.label_id.is_some() {
            vec![Label {
                id: row.label_id.unwrap(),
                name: row.label_name.clone().unwrap(),
            }]
        } else {
            vec![]
        };

        accum.push(TodoEntity {
            id: row.id,
            text: row.text.clone(),
            completed: row.completed,
            labels
        });
    }
    accum
        // .fold(vec![], |mut accum, current| {
        //     accum.push(TodoEntity { 
        //         id: current.id, 
        //         text: current.text.clone(), 
        //         completed: current.completed, 
        //         labels: vec![],
        //     });
        //     accum
        // })
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Validate)]
pub struct CreateTodo {
    #[validate(length(min = 1, message = "Can not be empty"))]
    #[validate(length(max = 100, message = "Over text length"))]
    text: String,
    labels: Vec<i32>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Validate)]
pub struct UpdateTodo {
    #[validate(length(min = 1, message = "Can not be empty"))]
    #[validate(length(max = 100, message = "Over text length"))]
    text: Option<String>,
    completed: Option<bool>,
    labels: Option<Vec<i32>>,
}

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
struct TodoFromRow {
    id: i32,
    text: String,
    completed: bool,
}

#[derive(Debug, Clone)]
pub struct TodoRepositoryForDb {
    pub pool: PgPool,
}

impl TodoRepositoryForDb {
    pub fn new(pool: PgPool) -> Self {
        TodoRepositoryForDb { pool }
    }
}

#[async_trait]
impl TodoRepository for TodoRepositoryForDb {
    async fn create(&self, payload: CreateTodo) -> anyhow::Result<TodoEntity> {
        let tx = self.pool.begin().await?;
        let row = sqlx::query_as::<_, TodoFromRow>(
            r#"
                INSERT INTO todos ( text, completed )
                VALUES ( $1, false )
                RETURNING *;
            "#,
        )
        .bind(payload.text.clone())
        .fetch_one(&self.pool)
        .await?;
        
        sqlx::query(
            r#"
                INSERT INTO todo_labels ( todo_id, label_id )
                SELECT $1, id
                FROM UNNEST ( $2 ) as t (id);
            "#,
        )
        .bind(row.id)
        .bind(payload.labels)
        .execute(&self.pool)
        .await?;

        tx.commit().await?;

        let todo = self.find(row.id).await?;
        Ok(todo)
    }

    async fn find(&self, id: i32) -> anyhow::Result<TodoEntity> {
        let items = sqlx::query_as::<_, TodoWithLabelFromRow>(
            r#"
                SELECT todos.*, labels.id as label_id, labels.name as label_name
                FROM todos
                LEFT OUTER JOIN todo_labels tl ON todos.id = tl.todo_id
                LEFT OUTER JOIN labels ON labels.id = tl.label_id
                WHERE todos.id = $1;
            "#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound(id),
            _ => RepositoryError::Unexpected(e.to_string()),
        })?;

        let todos = fold_entities(items);
        let todo = todos.first().ok_or(RepositoryError::NotFound(id))?;
        Ok(todo.clone())
    }

    async fn all(&self) -> anyhow::Result<Vec<TodoEntity>> {
        let items = sqlx::query_as::<_, TodoWithLabelFromRow>(
            r#"
                SELECT todos.*, labels.id as label_id, labels.name as label_name
                FROM todos
                LEFT OUTER JOIN todo_labels tl ON todos.id = tl.todo_id
                LEFT OUTER JOIN labels ON labels.id = tl.label_id
                ORDER BY todos.id desc;
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(fold_entities(items))
    }

    async fn update(&self, id: i32, payload: UpdateTodo) -> anyhow::Result<TodoEntity> {
        let tx = self.pool.begin().await?;

        let old_todo = self.find(id).await?;
        sqlx::query(
            r#"
                UPDATE todos SET text=$1, completed=$2
                WHERE id=$3
                RETURNING *
            "#,
        )
        .bind(payload.text.unwrap_or(old_todo.text))
        .bind(payload.completed.unwrap_or(old_todo.completed))
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        if let Some(labels) = payload.labels {
            // todos label update
            sqlx::query(
                r#"
                    DELETE FROM todo_labels WHERE todo_id = $1
                "#,
            )
            .bind(id)
            .execute(&self.pool)
            .await?;

            sqlx::query(
                r#"
                    INSERT INTO todo_labels ( todo_id, label_id )
                    SELECT $1, id
                    FROM UNNEST ( $2 ) AS t ( id );
                "#,
            )
            .bind(id)
            .bind(labels)
            .execute(&self.pool)
            .await?;
        }

        tx.commit().await?;
        let todo = self.find(id).await?;

        Ok(todo)
    }

    async fn delete(&self, id: i32) -> anyhow::Result<()> {
        let tx = self.pool.begin().await?;
        // todos label delete]
        sqlx::query(
            r#"
                DELETE FROM todo_labels WHERE todo_id = $1
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound(id),
            _ => RepositoryError::Unexpected(e.to_string()),
        })?;

        // todos delete
        sqlx::query(
            r#"
                DELETE FROM todos WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => RepositoryError::NotFound(id),
            _ => RepositoryError::Unexpected(e.to_string()),
        })?;

        tx.commit().await?;

        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "database-test")]
mod test {
    use super::*;
    use dotenv::dotenv;
    use sqlx::PgPool;
    use std::env;

    #[tokio::test]
    async fn crud_scenario() {
        dotenv().ok();
        let database_url = &env::var("DATABASE_URL").expect("undefined [DATABASE_URL]");
        let pool = PgPool::connect(database_url)
            .await
            .expect(&format!("fail connect database, url is [{}]", database_url));

        // label data prepare
        let label_name = String::from("test label");
        let optional_label = sqlx::query_as::<_, Label>(
            r#"
                SELECT * FROM labels WHERE name = $1
            "#,
        )
        .bind(label_name.clone())
        .fetch_optional(&pool)
        .await
        .expect("Failed to prepare label data.");
        let label_1 = if let Some(label) = optional_label {
            label
        } else {
            let label = sqlx::query_as::<_, Label>(
                r#"
                    INSERT INTO labels ( name )
                    VALUES ( $1 )
                    RETURNING *
                "#,
            )
            .bind(label_name)
            .fetch_one(&pool)
            .await
            .expect("Failed to insert label data.");
            label
        };

        let repository = TodoRepositoryForDb::new(pool.clone());
        let todo_text = "[crud_scenario] text";

        // create
        let created = repository
            .create(CreateTodo::new(todo_text.to_string(), vec![label_1.id]))
            .await
            .expect("[create] returned Err");
        assert_eq!(created.text, todo_text);
        assert!(!created.completed);
        assert_eq!(*created.labels.first().unwrap(), label_1);

        // find
        let todo = repository
            .find(created.id)
            .await
            .expect("[find] returned Err");
        assert_eq!(created, todo);

        // all
        let todos = repository.all().await.expect("[all] returned Err");
        let todo = todos.first().unwrap();
        assert_eq!(created, *todo);

        // update
        let update_text = "[crud_scenario] updated text";
        let todo = repository
            .update(
                todo.id,
                UpdateTodo {
                    text: Some(update_text.to_string()),
                    completed: Some(true),
                    labels: Some(vec![]),
                },
            )
            .await
            .expect("[update] returned Err");
        assert_eq!(created.id, todo.id);
        assert_eq!(update_text, todo.text);
        assert!(todo.labels.len() == 0);

        // delete
        let _ = repository
            .delete(todo.id)
            .await
            .expect("[delete] returned Err");
        let res = repository.find(todo.id).await; // expect not found err
        assert!(res.is_err());

        let todo_rows = sqlx::query(
            r#"
                SELECT * FROM todos WHERE id = $1
            "#,
        )
        .bind(todo.id)
        .fetch_all(&pool)
        .await
        .expect("[delete] todo_labels fetch error");
        assert!(todo_rows.len() == 0);

        let rows = sqlx::query(
            r#"
                SELECT * FROM todo_labels WHERE todo_id = $1
            "#,
        )
        .bind(todo.id)
        .fetch_all(&pool)
        .await
        .expect("[delete] todo_labels fetch error");
        assert!(rows.len() == 0)
    }
}

#[cfg(test)]
pub mod test_utils {
    use super::*;
    use anyhow::Context;
    use axum::async_trait;
    use std::{
        collections::HashMap,
        sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
    };

    impl TodoEntity {
        pub fn new(id: i32, text: String) -> Self {
            Self {
                id,
                text,
                completed: false,
                labels: vec![],
            }
        }
    }

    impl CreateTodo {
        pub fn new(text: String, labels: Vec<i32>) -> Self {
            Self { text, labels }
        }
    }
    
    type TodoDatas = HashMap<i32, TodoEntity>;

    #[derive(Debug, Clone)]
    pub struct TodoRepositoryForMemory {
        store: Arc<RwLock<TodoDatas>>,
    }

    impl TodoRepositoryForMemory {
        pub fn new() -> Self {
            TodoRepositoryForMemory {
                store: Arc::default(),
            }
        }
        fn write_store_ref(&self) -> RwLockWriteGuard<TodoDatas> {
            self.store.write().unwrap()
        }

        fn read_store_ref(&self) -> RwLockReadGuard<TodoDatas> {
            self.store.read().unwrap()
        }
    }

    #[async_trait]
    impl TodoRepository for TodoRepositoryForMemory {
        async fn create(&self, payload: CreateTodo) -> anyhow::Result<TodoEntity> {
            let mut store = self.write_store_ref();
            let id = (store.len() + 1) as i32;
            let todo = TodoEntity::new(id, payload.text.clone());
            store.insert(id, todo.clone());
            Ok(todo)
        }

        async fn find(&self, id: i32) -> anyhow::Result<TodoEntity> {
            let store = self.read_store_ref();
            let todo = store
                .get(&id)
                .cloned()
                .ok_or(RepositoryError::NotFound(id))?;
            Ok(todo)
        }
        
        async fn all(&self) -> anyhow::Result<Vec<TodoEntity>> {
            let store = self.read_store_ref();
            Ok(Vec::from_iter(store.values().cloned()))
        }

        async fn update(&self, id: i32, payload: UpdateTodo) -> anyhow::Result<TodoEntity> {
            let mut store = self.write_store_ref();
            let todo = store.get(&id).context(RepositoryError::NotFound(id))?;
            let text = payload.text.unwrap_or(todo.text.clone());
            let completed = payload.completed.unwrap_or(todo.completed);
            let todo = TodoEntity {
                id,
                text,
                completed,
                labels: vec![],
            };
            store.insert(id, todo.clone());
            Ok(todo)
        }

        async fn delete(&self, id: i32) -> anyhow::Result<()> {
            let mut store = self.write_store_ref();
            store.remove(&id).ok_or(RepositoryError::NotFound(id))?;
            Ok(())
        }
    }
    mod test {
        use super::*;

        #[tokio::test]
        async fn todo_crud_scenario() {
            let text = "todo text".to_string();
            let id = 1;
            let expected = TodoEntity::new(id, text.clone());

            // create
            let labels = vec![];
            let repository = TodoRepositoryForMemory::new();
            let todo = repository
                .create(CreateTodo { text, labels })
                .await
                .expect("failed create todo");
            assert_eq!(expected, todo);

            // find
            let todo = repository.find(todo.id).await.unwrap();
            assert_eq!(expected, todo);

            let todo = repository.all().await.expect("failed get all todo");
            assert_eq!(vec![expected], todo);

            let text = "update todo text".to_string();
            let todo = repository
                .update(
                    1,
                    UpdateTodo {
                        text: Some(text.clone()),
                        completed: Some(true),
                        labels: Some(vec![]),
                    },
                )
                .await
                .expect("failed update todo.");
            assert_eq!(
                TodoEntity {
                    id,
                    text,
                    completed: true,
                    labels: vec![],
                },
                todo
            );

            // delete
            let res = repository.delete(id).await;
            assert!(res.is_ok());
        }

        #[test]
        fn fold_entities_test() {
            let label_1 = Label {
                id: 1,
                name: String::from("label 1"),
            };
            let label_2 = Label {
                id: 2,
                name: String::from("label 2"),
            };
    
            let rows = vec![
                TodoWithLabelFromRow {
                    id: 1,
                    text: String::from("todo 1"),
                    completed: false,
                    label_id: Some(label_1.id),
                    label_name: Some(label_1.name.clone()),
                },
                TodoWithLabelFromRow {
                    id: 1,
                    text: String::from("todo 1"),
                    completed: false,
                    label_id: Some(label_2.id),
                    label_name: Some(label_2.name.clone()),
                },
                TodoWithLabelFromRow {
                    id: 2,
                    text: String::from("todo 2"),
                    completed: false,
                    label_id: Some(label_1.id),
                    label_name: Some(label_1.name.clone()),
                },
            ];
            let res = fold_entities(rows);
            assert_eq!(
                res,
                vec![
                    TodoEntity {
                        id: 1,
                        text: String::from("todo 1"),
                        completed: false,
                        labels: vec![
                            label_1.clone(),
                            label_2.clone(),
                        ],
                    },
                    TodoEntity {
                        id: 2,
                        text: String::from("todo 2"),
                        completed: false,
                        labels: vec![
                            label_1.clone(),
                        ]
                    },
                ]    
            );
        }
    
    }
}