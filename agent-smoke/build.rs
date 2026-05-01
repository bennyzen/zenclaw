// Required: re-emits esp-idf-sys's link args (--ldproxy-linker, etc.) into our final link.
// Without this call, ldproxy is invoked with an empty arg list and panics.
fn main() {
    embuild::espidf::sysenv::output();
}
