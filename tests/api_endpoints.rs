use reqwest::StatusCode;
use rust_srec::domain::{
    streamer::{Streamer, StreamerConfig},
    types::StreamerUrl,
};
use serde_json::json;

mod test_helpers;
use test_helpers::spawn_app;

#[tokio::test]
async fn post_streamer_returns_a_201_for_valid_form_data() {
    // Arrange
    let app = spawn_app().await;
    let client = &app.client;

    // Act
    let response = client
        .post(&format!("{}/api/streamers", &app.address))
        .json(&json!({
            "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "platform_config_id": "youtube",
            "config": {
                "live_title": "[{{title}}] {{streamer_name}} - {{time}}",
                "check_interval": 30,
                "cookies": null,
                "engine": "hls",
                "output": {
                    "path": "/path/to/save",
                    "format": "mp4"
                }
            }
        }))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(response.status(), StatusCode::CREATED);

    let saved = sqlx::query_as::<_, Streamer>("SELECT * FROM streamers")
        .fetch_one(&app.db_service.pool)
        .await
        .expect("Failed to fetch saved streamer.");

    assert_eq!(saved.url.as_ref(), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    assert_eq!(saved.platform_config_id, "youtube");
}

#[tokio::test]
async fn get_streamers_returns_a_200_and_a_list_of_streamers() {
    // Arrange
    let app = spawn_app().await;
    let client = &app.client;

    // Create a couple of streamers to be listed
    let streamer1 = Streamer {
        id: "test-streamer-1".to_string(),
        url: StreamerUrl("https://www.twitch.tv/test1".to_string()),
        platform_config_id: "twitch".to_string(),
        config: StreamerConfig::default(),
        ..Default::default()
    };
    let streamer2 = Streamer {
        id: "test-streamer-2".to_string(),
        url: StreamerUrl("https://www.youtube.com/c/test2".to_string()),
        platform_config_id: "youtube".to_string(),
        config: StreamerConfig::default(),
        ..Default::default()
    };
    app.db_service.streamers().create(&streamer1).await.unwrap();
    app.db_service.streamers().create(&streamer2).await.unwrap();

    // Act
    let response = client
        .get(&format!("{}/api/streamers", &app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);

    let streamers: Vec<Streamer> = response.json().await.unwrap();
    assert_eq!(streamers.len(), 2);
}

#[tokio::test]
async fn get_streamer_by_id_returns_a_200_and_the_streamer() {
    // Arrange
    let app = spawn_app().await;
    let client = &app.client;

    let streamer = Streamer {
        id: "test-streamer".to_string(),
        url: StreamerUrl("https://www.twitch.tv/test".to_string()),
        platform_config_id: "twitch".to_string(),
        config: StreamerConfig::default(),
        ..Default::default()
    };
    app.db_service.streamers().create(&streamer).await.unwrap();

    // Act
    let response = client
        .get(&format!("{}/api/streamers/{}", &app.address, streamer.id))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);

    let fetched_streamer: Streamer = response.json().await.unwrap();
    assert_eq!(fetched_streamer.id, streamer.id);
}

#[tokio::test]
async fn put_streamer_returns_a_200_and_updates_the_streamer() {
    // Arrange
    let app = spawn_app().await;
    let client = &app.client;

    let streamer = Streamer {
        id: "test-streamer".to_string(),
        url: StreamerUrl("https://www.twitch.tv/test".to_string()),
        platform_config_id: "twitch".to_string(),
        config: StreamerConfig::default(),
        ..Default::default()
    };
    app.db_service.streamers().create(&streamer).await.unwrap();

    // Act
    let response = client
        .put(&format!("{}/api/streamers/{}", &app.address, streamer.id))
        .json(&json!({
            "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "platform_config_id": "youtube",
            "config": {
                "live_title": "new title",
            }
        }))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(response.status(), StatusCode::OK);

    let updated_streamer = app.db_service.streamers().find_by_id(&streamer.id).await.unwrap().unwrap();
    assert_eq!(updated_streamer.url.as_ref(), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    assert_eq!(updated_streamer.platform_config_id, "youtube");
    assert_eq!(updated_streamer.config.live_title.unwrap(), "new title");
}

#[tokio::test]
async fn delete_streamer_returns_a_204() {
    // Arrange
    let app = spawn_app().await;
    let client = &app.client;

    let streamer = Streamer {
        id: "test-streamer".to_string(),
        url: StreamerUrl("https://www.twitch.tv/test".to_string()),
        platform_config_id: "twitch".to_string(),
        config: StreamerConfig::default(),
        ..Default::default()
    };
    app.db_service.streamers().create(&streamer).await.unwrap();

    // Act
    let response = client
        .delete(&format!("{}/api/streamers/{}", &app.address, streamer.id))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let deleted_streamer = app.db_service.streamers().find_by_id(&streamer.id).await.unwrap();
    assert!(deleted_streamer.is_none());
}