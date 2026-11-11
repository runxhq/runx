use std::alloc;
use std::io::Write;

use cap::Cap;
use runx_js_worker::protocol::JAVASCRIPT_HEAP_BYTES;

#[global_allocator]
static ALLOCATOR: Cap<alloc::System> = Cap::new(alloc::System, JAVASCRIPT_HEAP_BYTES as usize);

fn main() {
    if let Err(error) = runx_js_worker::serve(|| ALLOCATOR.allocated()) {
        let mut message = error.to_string();
        let mut end = message.len().min(4096);
        while !message.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        message.truncate(end);
        message.push('\n');
        let _ = std::io::stderr().write_all(message.as_bytes());
        std::process::exit(70);
    }
}
