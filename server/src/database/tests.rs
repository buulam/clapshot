use tracing_test::traced_test;
use crate::database::*;

use models::{User, Video, VideoInsert, Message, MessageInsert, Comment, CommentInsert};


fn _dump_db(conn: &mut PooledConnection) {
    println!("================ dump_db ================");

    conn.transaction(|conn| {
        let videos = Video::get_all(conn, DBPaging::default()).unwrap();
        println!("----- Videos -----");
        for v in videos { println!("----\n{:#?}", v);}

        let comments = Comment::get_all(conn, DBPaging::default()).unwrap();
        println!("----- Comments -----");
        for c in comments { println!("----\n{:#?}", c);}

        let messages = Message::get_all(conn, DBPaging::default()).unwrap();
        println!("----- Messages -----");
        for m in messages { println!("----\n{:#?}", m);}

        DBResult::Ok(())
    }).unwrap();
    println!("=========================================");
}

/// Create a temporary database and populate it for testing.
///
/// Contents are roughly as follows:
/// ```text
/// <Video(id=B1DE0 orig_filename=test0.mp4 added_by_userid=user.num1 ...)>
/// <Video(id=1111 orig_filename=test1.mp4 added_by_userid=user.num2 ...)>
/// <Video(id=22222 orig_filename=test2.mp4 added_by_userid=user.num1 ...)>
/// <Video(id=B1DE3 orig_filename=test3.mp4 added_by_userid=user.num2 ...)>
/// <Video(id=B1DE4 orig_filename=test4.mp4 added_by_userid=user.num1 ...)>
/// <Comment(id='1' video=HASH0 parent=None user_id='user.num1' comment='Comment 0' has-drawing=True ...)>
/// <Comment(id='2' video=1111 parent=None user_id='user.num2' comment='Comment 1' has-drawing=True ...)>
/// <Comment(id='3' video=22222 parent=None user_id='user.num1' comment='Comment 2' has-drawing=True ...)>
/// <Comment(id='4' video=HASH0 parent=None user_id='user.num2' comment='Comment 3' has-drawing=True ...)>
/// <Comment(id='5' video=1111 parent=None user_id='user.num1' comment='Comment 4' has-drawing=True ...)>
/// <Comment(id='6' video=HASH0 parent=1 user_id='user.num2' comment='Comment 5' has-drawing=True ...)>
/// <Comment(id='7' video=HASH0 parent=1 user_id='user.num1' comment='Comment 6' has-drawing=True ...)>
/// ```
pub fn make_test_db() -> (std::sync::Arc<DB>, assert_fs::TempDir, Vec<Video>, Vec<Comment>)
{
    println!("--- make_test_db");

    let data_dir = assert_fs::TempDir::new().unwrap();
    std::fs::create_dir(&data_dir.path().join("incoming")).ok();

    let db = std::sync::Arc::new(DB::open_db_file(data_dir.join("clapshot.sqlite").as_path()).unwrap());
    let conn = &mut db.conn().unwrap();

    for m in db.pending_migration_names().unwrap() {
        db.apply_migration(conn, &m).unwrap();
    }

    // Make some videos
    let hashes = vec!["B1DE0", "11111", "22222", "B1DE3", "B1DE4"];
    let mkvid = |i: usize| {

        let user_id = format!("user.num{}", 1 + i % 2);
        let username = format!("User Number{}", 1 + i % 2);
        let user = User::get_or_create(conn, &user_id, Some(&username)).expect("Failed to create user");

        let v = VideoInsert {
            id: hashes[i].to_string(),
            user_id: user.id.clone(),
            orig_filename: Some(format!("test{}.mp4", i)),
            title: Some(format!("test{}.mp4", i)),
            recompression_done: Some(chrono::NaiveDateTime::default()),
            thumb_sheet_cols: Some(i as i32),
            thumb_sheet_rows: Some(i as i32),
            total_frames: Some((i * 1000) as i32),
            duration: Some((i * 100) as f32),
            fps: Some(format!("{}", i * i)),
            raw_metadata_all: Some(format!("{{all: {{video: {}}}}}", i)),
        };
        Video::insert(conn, &v).expect("Failed to insert video");
        Video::get(conn, &v.id.into()).expect("Failed to get video")
    };
    let videos = (0..5).map(mkvid).collect::<Vec<_>>();

    // Make some comments
    let mut mkcom = |i: usize, vid: &str, parent_id: Option<i32>| {
        let c = CommentInsert {
            video_id: vid.to_string(),
            parent_id,
            timecode: None,
            user_id: Some(format!("user.num{}", 1 + i % 2)),
            username_ifnull: format!("User Number{}", 1 + i % 2),
            comment: format!("Comment {}", i),
            drawing: Some(format!("drawing_{}.webp", i)),
        };
        let c = Comment::insert(conn, &c).expect("Failed to insert comment");
        let dp = data_dir.join("videos").join(vid).join("drawings");
        std::fs::create_dir_all(&dp).expect("Failed to create drawing directory");
        std::fs::write(dp.join(&c.drawing.clone().unwrap()), "IMAGE_DATA").expect("Failed to write drawing");
        c
    };
    let mut comments = (0..5)
        .map(|i| mkcom(i, &videos[i % 3].id, None))
        .collect::<Vec<_>>();
    let more_comments = (5..5 + 2)
        .map(|i| mkcom(i, &comments[0].video_id, Some(comments[0].id)))
        .collect::<Vec<_>>();
    comments.extend(more_comments);

    // Add another comment (#8) with empty drawing (caused an error at one point)
    let c = CommentInsert {
        video_id: videos[0].id.clone(),
        parent_id: None,
        timecode: None,
        user_id: Some("user.num1".to_string()),
        username_ifnull: "User Number1".to_string(),
        comment: "Comment_with_empty_drawing".to_string(),
        drawing: Some("".into()),
    };
    let cmt = models::Comment::insert(conn, &c).expect("Failed to insert comment");
    comments.push(cmt);

    // _dump_db(conn);   // Uncomment to debug database contents
    (db, data_dir, videos, comments)
}


#[test]
#[traced_test]
fn test_pagination() -> anyhow::Result<()> {
    let (db, _data_dir, _videos, comments) = make_test_db();
    let conn = &mut db.conn()?;

    // Test pagination of comments
    let mut res = Comment::get_all(conn, DBPaging { page_num: 0, page_size: 3.try_into()? })?;
    println!("---- page 0, 3");
    println!("res: {:#?}", res);

    assert_eq!(res.len(), 3);
    assert_eq!(res[0].id, comments[0].id);
    assert_eq!(res[1].id, comments[1].id);
    assert_eq!(res[2].id, comments[2].id);

    res = Comment::get_all(conn, DBPaging { page_num: 1, page_size: 3.try_into()? })?;
    println!("---- page 1, 3");
    println!("res: {:#?}", res);
    assert_eq!(res.len(), 3);
    assert_eq!(res[0].id, comments[3].id);
    assert_eq!(res[1].id, comments[4].id);
    assert_eq!(res[2].id, comments[5].id);

    res = Comment::get_all(conn, DBPaging { page_num: 2, page_size: 3.try_into()? })?;
    println!("---- page 2, 3");
    println!("res: {:#?}", res);
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].id, comments[6].id);
    assert_eq!(res[1].id, comments[7].id);

    Ok(())
}


// ----------------------------------------------------------------------------


#[test]
#[traced_test]
fn test_fixture_state() -> anyhow::Result<()>
{
    let (db, _data_dir, videos, comments) = make_test_db();
    let conn = &mut db.conn()?;

    // First 5 comments have no parent, last 2 have parent_id=1
    for i in 0..5 { assert!(comments[i].parent_id.is_none()); }
    for i in 5..5 + 2 { assert_eq!(comments[i].parent_id, Some(comments[0].id)); }

    // Video #0 has 3 comments, video #1 has 2, video #2 has 1
    assert_eq!(comments[0].video_id, comments[3].video_id);
    assert_eq!(comments[0].video_id, comments[5].video_id);
    assert_eq!(comments[0].video_id, comments[6].video_id);
    assert_eq!(comments[0].video_id, videos[0].id);
    assert_eq!(comments[1].video_id, comments[4].video_id);
    assert_eq!(comments[1].video_id, videos[1].id);
    assert_eq!(comments[2].video_id, videos[2].id);

    // Read entries from database and check that they match definitions
    for v in videos.iter() {
        assert_eq!(Video::get(conn, &v.id)?.id, v.id);
        let comments = Comment::get_by_video(conn, &v.id, DBPaging::default())?;
        assert_eq!(comments.len(), match v.id.as_str() {
            "B1DE0" => 5,
            "11111" => 2,
            "22222" => 1,
            "B1DE3" => 0,
            "B1DE4" => 0,
            _ => panic!("Unexpected video id"),
        });
    }
    for c in comments.iter() {
        assert_eq!(models::Comment::get(conn, &c.id)?.id, c.id);
        assert_eq!(models::Comment::get(conn, &c.id)?.comment, c.comment);
    }

    // Check that we can get videos by user
    assert_eq!(models::Video::get_by_user(conn, "user.num1", DBPaging::default())?.len(), 3);
    assert_eq!(models::Video::get_by_user(conn, "user.num2", DBPaging::default())?.len(), 2);
    Ok(())
}


#[test]
#[traced_test]
fn test_comment_delete() -> anyhow::Result<()> {
    let (db, _data_dir, _vid, com) = make_test_db();
    let conn = &mut db.conn()?;

    assert_eq!(Comment::get_by_video(conn, &com[1].video_id, DBPaging::default())?.len(), 2, "Video should have 2 comments before deletion");

    // Delete comment #2 and check that it was deleted, and nothing else
    models::Comment::delete(&mut db.conn()?, &com[1].id)?;
    for c in com.iter() {
        if c.id == com[1].id {
            assert!(matches!(models::Comment::get(conn, &c.id).unwrap_err() , DBError::NotFound()), "Comment should be deleted");
        } else {
            assert_eq!(models::Comment::get(conn, &c.id)?.id, c.id, "Deletion removed wrong comment(s)");
        }
    }

    // Check that video still has 1 comment
    assert_eq!(Comment::get_by_video(conn, &com[1].video_id, DBPaging::default())?.len(), 1, "Video should have 1 comment left");

    // Delete last, add a new one and check for ID reuse
    models::Comment::delete(&mut db.conn()?, &com[6].id)?;
    let c = CommentInsert {
        video_id: com[1].video_id.clone(),
        parent_id: None,
        user_id: com[1].user_id.clone(),
        username_ifnull: "name".to_string(),
        comment: "re-add".to_string(),
        timecode: None,
        drawing: None,
    };
    let new_id = models::Comment::insert(conn, &c)?.id;
    assert_ne!(new_id, com[6].id, "Comment ID was re-used after deletion. This would mix up comment threads in the UI.");
    Ok(())
}

#[test]
#[traced_test]
fn test_rename_video() -> anyhow::Result<()> {
    let (db, _data_dir, _vid, _com) = make_test_db();
    let conn = &mut db.conn()?;

    // Rename video #1
    let new_name = "New name";
    Video::rename(conn, "11111", new_name)?;

    // Check that video #1 has new name
    let v = Video::get(conn, &"11111".into())?;
    assert_eq!(v.title, Some(new_name.into()));

    // Check that video #2 still has old name
    let v = Video::get(conn, &"22222".into())?;
    assert_ne!(v.title, Some(new_name.into()));

    Ok(())
}


#[test]
#[traced_test]
fn test_user_messages() -> anyhow::Result<()> {
    let (db, _data_dir, _vid, _com) = make_test_db();
    let conn = &mut db.conn()?;

    // Add a message to user #1
    let msgs = [
        MessageInsert {
            user_id: "user.num1".into(),
            message: "message1".into(),
            event_name: "info".into(),
            video_id: Some("HASH0".into()),
            comment_id: None,
            details: "".into(),
            seen: false,
        },
        MessageInsert {
            user_id: "user.num1".into(),
            message: "message2".into(),
            event_name: "oops".into(),
            video_id: Some("HASH0".into()),
            comment_id: None,
            details: "STACKTRACE".into(),
            seen: false,
        },
        MessageInsert {
            user_id: "user.num2".into(),
            message: "message3".into(),
            event_name: "info".into(),
            video_id: None,
            comment_id: None,
            details: "".into(),
            seen: false,
        },
    ];

    let mut new_msgs = vec![];
    for i in 0..msgs.len() {
        let new_msg = Message::insert(conn, &msgs[i])?;
        assert_eq!(new_msg.user_id, msgs[i].user_id);
        assert_eq!(new_msg.message, msgs[i].message);

        let a = serde_json::to_value(Message::get(conn, &new_msg.id)?.to_proto3())?;
        let b = serde_json::to_value(new_msg.to_proto3())?;
        assert_eq!(a,b);

        assert!(!Message::get(conn, &new_msg.id)?.seen);
        new_msgs.push(new_msg);
    }

    // Correctly count messages
    assert_eq!(Message::get_by_user(conn, "user.num1", DBPaging::default())?.len(), 2);
    assert_eq!(Message::get_by_user(conn, "user.num2", DBPaging::default())?.len(), 1);

    // Mark message #2 as seen
    Message::set_seen(conn, new_msgs[1].id, true)?;
    assert!(Message::get(conn, &new_msgs[1].id)?.seen);

    // Delete & recount
    Message::delete(conn, &new_msgs[2].id)?;
    Message::delete(conn, &new_msgs[0].id)?;
    assert_eq!(Message::get_by_user(conn, "user.num1", DBPaging::default())?.len(), 1);
    assert_eq!(Message::get_by_user(conn, "user.num2", DBPaging::default())?.len(), 0);

    Ok(())
}

#[test]
#[traced_test]
fn test_transaction_rollback() -> anyhow::Result<()> {
    let (db, _data_dir, vid, _com) = make_test_db();
    let conn = &mut db.conn()?;

    assert_eq!(Video::get_all(conn, DBPaging::default()).unwrap().len(), vid.len());

    conn.transaction::<(), _, _>(|conn| {
        Video::delete(conn, &vid[0].id).unwrap();
        assert_eq!(Video::get_all(conn, DBPaging::default()).unwrap().len(), vid.len()-1);
        Err(diesel::result::Error::RollbackTransaction)
    }).ok();

    assert_eq!(Video::get_all(conn, DBPaging::default()).unwrap().len(), vid.len());
    Ok(())
}

#[test]
#[traced_test]
fn test_transaction_commit() -> anyhow::Result<()> {
    let (db, _data_dir, vid, _com) = make_test_db();
    let conn = &mut db.conn()?;

    assert_eq!(Video::get_all(conn, DBPaging::default()).unwrap().len(), vid.len());
    conn.transaction::<(), _, _>(|conn| {
        Video::delete(conn, &vid[0].id).unwrap();
        assert_eq!(Video::get_all(conn, DBPaging::default()).unwrap().len(), vid.len()-1);
        DBResult::Ok(())
    }).unwrap();
    assert_eq!(Video::get_all(conn, DBPaging::default()).unwrap().len(), vid.len()-1);

    Ok(())
}
