use super::*;

#[test]
fn cancellation_waits_for_completion_and_is_delivered_once() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&events);
    let mut session = Session {
        handler: move |event| {
            observed.borrow_mut().push(event);
            Ok(if event == ProgressEvent::Tick {
                ProgressUpdate::Finished
            } else {
                ProgressUpdate::KeepOpen
            })
        },
        phase: Phase::Waiting,
    };
    assert!(matches!(
        session.handle(ProgressEvent::Tick).unwrap(),
        ProgressUpdate::KeepOpen
    ));
    session.handle(ProgressEvent::Created).unwrap();
    assert!(matches!(
        session.handle(ProgressEvent::CancelRequested).unwrap(),
        ProgressUpdate::KeepOpen
    ));
    session.handle(ProgressEvent::CancelRequested).unwrap();
    assert!(matches!(
        session.handle(ProgressEvent::Tick).unwrap(),
        ProgressUpdate::Finished
    ));
    session.handle(ProgressEvent::Tick).unwrap();
    assert_eq!(
        *events.borrow(),
        [
            ProgressEvent::Created,
            ProgressEvent::CancelRequested,
            ProgressEvent::Tick
        ]
    );
}

#[test]
fn controller_failure_is_propagated() {
    let mut session = Session {
        handler: |_| Err(io::Error::new(io::ErrorKind::BrokenPipe, "worker stopped")),
        phase: Phase::Waiting,
    };
    assert_eq!(
        session.handle(ProgressEvent::Created).err().unwrap().kind(),
        io::ErrorKind::BrokenPipe
    );
}
