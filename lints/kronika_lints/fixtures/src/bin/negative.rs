#![allow(dead_code)]

#[derive(Clone, Copy)]
enum Shared {
    One,
    Two,
}

fn nonidentity(value: Shared) -> Shared {
    match value {
        Shared::One => Shared::Two,
        Shared::Two => Shared::One,
    }
}

fn guarded(value: Shared, keep: bool) -> Shared {
    match value {
        Shared::One if keep => Shared::One,
        Shared::One => Shared::One,
        Shared::Two => Shared::Two,
    }
}

enum Payload {
    One(u8),
    Two,
}

fn payload(value: Payload) -> Payload {
    match value {
        Payload::One(inner) => Payload::One(inner),
        Payload::Two => Payload::Two,
    }
}

fn consume(cancelled: &(impl Fn() -> bool + ?Sized)) -> bool {
    cancelled()
}

fn direct(cancelled: &dyn Fn() -> bool) -> bool {
    consume(cancelled)
}

fn extra_work(cancelled: &dyn Fn() -> bool) -> bool {
    consume(&|| {
        let cancelled = cancelled();
        cancelled && std::hint::black_box(true)
    })
}

fn main() {}
