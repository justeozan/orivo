;; A component whose only import is a WASI function.
;;
;; Orivo's host provides no WASI — general or otherwise — so this is the
;; smallest package that must be refused before instantiation. The imported
;; instance has to declare a function: an empty instance type asks the host for
;; nothing, and a linker can satisfy nothing trivially.
;;
;; A hand-written component is cheaper than a second Rust guest, and the refusal
;; it proves is one of the load-bearing ones.
(component
  (type $monotonic-clock (instance
    (export "now" (func (result u64)))
  ))
  (import "wasi:clocks/monotonic-clock@0.2.0" (instance (type $monotonic-clock)))
)
