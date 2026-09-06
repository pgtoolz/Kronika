#![allow(dead_code)]

#[derive(Clone, Copy)]
enum Shared {
    One,
    Two,
}

type Alias = Shared;

fn identity(value: Shared) -> Alias {
    match value {
        Shared::One => Alias::One,
        Shared::Two => Alias::Two,
    }
}

fn consume<F: Fn() -> bool>(cancelled: &F) -> bool {
    cancelled()
}

fn forwarding(cancelled: &dyn Fn() -> bool) -> bool {
    consume(&|| cancelled())
}

fn discarded_error() {
    std::fs::metadata(".").ok();
}

fn first_mapping(value: Shared) -> u8 {
    match value {
        Shared::One => 1,
        Shared::Two => 2,
    }
}

fn second_mapping(value: Shared, enabled: bool) -> Option<u8> {
    if !enabled {
        return None;
    }
    Some(match value {
        Shared::One => 1,
        Shared::Two => 2,
    })
}

fn clamp_add(base: u32, delta: u32, limit: u32) -> u32 {
    let sum = base.saturating_add(delta);
    if sum > limit { limit } else { sum }
}

fn bounded_sum(left: u32, right: u32, maximum: u32) -> u32 {
    let sum = left.saturating_add(right);
    if sum > maximum { maximum } else { sum }
}

fn render(text: &str, wrap: bool, color: bool) -> usize {
    text.len() + usize::from(wrap) + usize::from(color)
}

fn main() {
    let _ = render("fixture", true, false);
}
