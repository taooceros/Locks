alias r := run2

default_cs := "1000,3000"
default_non_cs := "0"

default_arg := "--cs " + default_cs + " --non-cs " + default_non_cs

profile := "debug"

profile_arg := if profile == "release-with-debug" {"--profile=release-with-debug"} else {""}

build:
    #!/usr/bin/env zsh
    cd ./rust
    cargo build {{profile_arg}}

run2 experiment="counter-proportional" locks="" *additional_arg=default_arg: build
    #!/usr/bin/env zsh
    cd ./rust

    cargo run {{profile_arg}} -- d-lock2  {{ if locks == "" {""} else {"--lock-targets " + locks} }} {{experiment}} {{additional_arg}}

run1 experiment locks="" *additional_arg=default_arg: build
    #!/usr/bin/env zsh
    cd ./rust

    cargo run {{profile_arg}} -- d-lock1  {{ if locks == "" {""} else {"--lock-targets " + locks} }} counter-proportional {{additional_arg}}


queue2 locks="": (run2 "queue" locks "")
    true