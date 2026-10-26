use super::*;

#[test]
fn transfer_saves_to_destination_tail_preserves_ids_and_reopens() {
    let (dir, mut session) = fixture();
    until(&mut session, |s| s.loaded);
    for _ in 0..3 {
        session.create_group();
        until(&mut session, |s| s.can_save());
    }
    let source_a = GroupId::new(1).unwrap();
    let source_b = GroupId::new(2).unwrap();
    let destination = GroupId::new(3).unwrap();
    let sources: Vec<_> = (0..4)
        .map(|index| dir.join(format!("transfer-{index}.txt")))
        .collect();
    for path in &sources {
        fs::write(path, b"transfer leaves source bytes untouched").unwrap();
    }
    assert!(session.add_references(
        source_a,
        vec![source_reference(&sources[0]), source_reference(&sources[1])]
    ));
    until(&mut session, |s| s.can_save());
    assert!(session.add_references(source_b, vec![source_reference(&sources[2])]));
    until(&mut session, |s| s.can_save());
    assert!(session.add_references(destination, vec![source_reference(&sources[3])]));
    until(&mut session, |s| s.can_save());

    let next_item = session.config.as_ref().unwrap().next_item_id;
    let moved = session.config.as_ref().unwrap().groups[0].items[0].clone();
    assert!(session.transfer_reference(source_a, moved.id, destination));
    until(&mut session, |s| s.can_save());
    let expected = session.config.as_ref().unwrap().clone();
    assert_eq!(expected.next_item_id, next_item);
    assert_eq!(expected.groups[0].items.len(), 1);
    assert_eq!(expected.groups[0].items[0].id, ItemId::new(2).unwrap());
    assert_eq!(expected.groups[1].items[0].id, ItemId::new(3).unwrap());
    assert_eq!(
        expected.groups[2]
            .items
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        vec![ItemId::new(4).unwrap(), moved.id]
    );
    assert_eq!(expected.groups[2].items[1].target, moved.target);
    for path in &sources {
        assert_eq!(fs::read(path).unwrap(), b"transfer leaves source bytes untouched");
    }
    session.close();
    until(&mut session, Session::should_destroy);

    let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
    until(&mut reopened, |s| s.loaded);
    assert_eq!(reopened.config.as_ref().unwrap().as_ref(), expected.as_ref());
    reopened.close();
    until(&mut reopened, Session::should_destroy);
}

#[test]
fn transfer_rejects_invalid_or_busy_requests_without_config_changes() {
    let (dir, mut session) = fixture();
    until(&mut session, |s| s.loaded);
    for _ in 0..2 {
        session.create_group();
        until(&mut session, |s| s.can_save());
    }
    let source = GroupId::new(1).unwrap();
    let destination = GroupId::new(2).unwrap();
    let first = source_reference(&dir.join("transfer-reject.txt"));
    let second = source_reference(&dir.join("transfer-reject-other.txt"));
    assert!(session.add_references(source, vec![first.clone(), second]));
    until(&mut session, |s| s.can_save());
    assert!(session.add_references(destination, vec![first]));
    until(&mut session, |s| s.can_save());

    let item = ItemId::new(2).unwrap();
    let unchanged = session.config.as_ref().unwrap().clone();
    assert!(!session.transfer_reference(source, item, source));
    assert!(!session.transfer_reference(source, item, GroupId::new(999).unwrap()));
    assert!(!session.transfer_reference(source, ItemId::new(1).unwrap(), destination)); // Duplicate path.
    assert_eq!(session.config.as_ref().unwrap().as_ref(), unchanged.as_ref());

    session.writable = false;
    assert!(!session.transfer_reference(source, item, destination));
    assert_eq!(session.config.as_ref().unwrap().as_ref(), unchanged.as_ref());
    session.writable = true;

    session.rename_group(source, "保存期间".into());
    assert!(!session.can_save());
    let saving_snapshot = session.config.as_ref().unwrap().clone();
    assert!(!session.transfer_reference(source, item, destination));
    assert_eq!(
        session.config.as_ref().unwrap().as_ref(),
        saving_snapshot.as_ref()
    );
    until(&mut session, |s| s.can_save());
    session.close();
    until(&mut session, Session::should_destroy);
}

#[test]
fn failed_transfer_save_restores_groups_and_preserves_removal_history() {
    let (dir, mut session) = fixture();
    until(&mut session, |s| s.loaded);
    for _ in 0..2 {
        session.create_group();
        until(&mut session, |s| s.can_save());
    }
    let source = GroupId::new(1).unwrap();
    let destination = GroupId::new(2).unwrap();
    let paths: Vec<_> = (0..5)
        .map(|index| dir.join(format!("transfer-fail-{index}.txt")))
        .collect();
    for path in &paths {
        fs::write(path, b"unchanged source fixture").unwrap();
    }
    assert!(session.add_references(
        source,
        paths.iter().take(4).map(|p| source_reference(p)).collect()
    ));
    until(&mut session, |s| s.can_save());
    assert!(session.add_references(destination, vec![source_reference(&paths[4])]));
    until(&mut session, |s| s.can_save());
    assert!(session.remove_reference(source, ItemId::new(4).unwrap()));
    until(&mut session, |s| s.can_save());
    assert!(session.can_undo_removal());

    let before = session.config.as_ref().unwrap().clone();
    let saved_bytes = fs::read(dir.join("config.json")).unwrap();
    fs::write(dir.join("config.pending"), b"external pending fixture").unwrap();
    assert!(session.transfer_reference(source, ItemId::new(2).unwrap(), destination));
    until(&mut session, |s| !s.tracker.is_saving());
    assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
    assert_eq!(fs::read(dir.join("config.json")).unwrap(), saved_bytes);
    assert_eq!(
        fs::read(dir.join("config.pending")).unwrap(),
        b"external pending fixture"
    );
    assert!(session.last_removal.is_some());
    assert!(!session.can_undo_removal()); // Save failure has enabled read-only protection.
    assert!(session.details.contains("组间移动保存失败"));
    for path in &paths {
        assert_eq!(fs::read(path).unwrap(), b"unchanged source fixture");
    }
    session.close();
    until(&mut session, Session::should_destroy);
}

#[test]
fn disconnected_worker_rolls_back_transfer_before_submit_and_during_save() {
    for during_save in [false, true] {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        for _ in 0..2 {
            session.create_group();
            until(&mut session, |s| s.can_save());
        }
        let source = GroupId::new(1).unwrap();
        let destination = GroupId::new(2).unwrap();
        let path = dir.join("transfer-disconnect.txt");
        fs::write(&path, b"disconnect does not touch source").unwrap();
        assert!(session.add_references(source, vec![source_reference(&path)]));
        until(&mut session, |s| s.can_save());
        let before = session.config.as_ref().unwrap().clone();
        let config_bytes = fs::read(dir.join("config.json")).unwrap();

        session.worker.submit(Command::Shutdown).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match session.worker.poll() {
                Err(TryRecvError::Disconnected) => break,
                Ok(Event::Stopped) | Err(TryRecvError::Empty) => {}
                event => panic!("unexpected shutdown event: {event:?}"),
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        if during_save {
            transfer_reference(
                Arc::make_mut(session.config.as_mut().unwrap()),
                source,
                ItemId::new(1).unwrap(),
                destination,
            )
            .unwrap();
            session.pending_transfer = Some((source, ItemId::new(1).unwrap(), destination, 0));
            session.tracker.begin().unwrap();
            assert!(session.drain());
            assert!(session.status.contains("未确认的组间移动已回退"));
        } else {
            assert!(!session.transfer_reference(source, ItemId::new(1).unwrap(), destination));
            assert!(session.status.contains("无法提交保存"));
            session.drain();
        }
        assert!(session.pending_transfer.is_none());
        assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
        assert_eq!(fs::read(dir.join("config.json")).unwrap(), config_bytes);
        assert_eq!(fs::read(path).unwrap(), b"disconnect does not touch source");
        session.close();
        assert!(session.should_destroy());
    }
}
