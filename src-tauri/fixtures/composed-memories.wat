;; Two components composed inside one, each with its own linear memory, and a
;; string crossing between them.
;;
;; A call like that is what makes Wasmtime synthesise a FACT adapter module, and
;; such an adapter imports *both* memories (`wasmtime-environ/src/fact.rs`). The
;; adapter is then validated with the engine's own features
;; (`wasmtime/src/compile.rs` -> `component/translate/adapt.rs`), and that
;; validation is an `expect`: an engine with multi-memory turned off does not
;; refuse this component, it panics. Which is why this fixture exists.
(component
  (component $inner
    (core module $m
      (memory (export "mem") 1)
      (func (export "run") (param i32 i32) (result i32) unreachable)
      (func (export "realloc") (param i32 i32 i32 i32) (result i32) unreachable)
    )
    (core instance $i (instantiate $m))
    (func (export "echo") (param "s" string) (result string)
      (canon lift (core func $i "run")
        (memory $i "mem")
        (realloc (func $i "realloc"))
        string-encoding=utf8))
  )
  (component $outer
    (import "echo" (func $echo (param "s" string) (result string)))
    (core module $m
      (import "host" "echo" (func $echo (param i32 i32 i32)))
      (memory (export "mem") 1)
      (func (export "go") (call $echo (i32.const 0) (i32.const 0) (i32.const 0)))
      (func (export "realloc") (param i32 i32 i32 i32) (result i32) unreachable)
    )
    (core module $helper
      (memory (export "mem") 1)
      (func (export "realloc") (param i32 i32 i32 i32) (result i32) unreachable)
    )
    (core instance $h (instantiate $helper))
    (core func $lowered
      (canon lower (func $echo)
        (memory $h "mem")
        (realloc (func $h "realloc"))
        string-encoding=utf8))
    (core instance $i (instantiate $m (with "host" (instance (export "echo" (func $lowered))))))
    (func (export "go") (canon lift (core func $i "go")))
  )
  (instance $a (instantiate $inner))
  (instance $b (instantiate $outer (with "echo" (func $a "echo"))))
  (func (export "go") (alias export $b "go"))
)
