set positional-arguments

alias r := run2

default_cs := "1000,3000"
default_non_cs := "0"

default_arg := "--cs " + default_cs + " --non-cs " + default_non_cs

build:
    #!/usr/bin/env zsh
    cd ./rust
    cargo build --release

run2 locks="" *additional_arg=default_arg: build
    #!/usr/bin/env zsh
    cd ./rust

    cargo run --release -- d-lock2  {{ if locks == "" {""} else {"--lock-targets " + locks} }} counter-proportional {{additional_arg}}

run1 locks="" *additional_arg=default_arg: build
    #!/usr/bin/env zsh
    cd ./rust

    cargo run --release -- d-lock1  {{ if locks == "" {""} else {"--lock-targets " + locks} }} counter-proportional {{additional_arg}}