pub fn exit() -> ! {
    std::process::exit(0);
}

pub fn echo(content: impl AsRef<str>) -> (){
    println!("{}",content.as_ref());
}
