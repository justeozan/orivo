;; A component whose core module declares a 64-bit linear memory.
;;
;; Orivo's plugin ABI needs one ordinary 32-bit memory per core module, and a
;; 64-bit one is the doorway to Wasmtime's "growth exceeds address space" path —
;; the one place it tells a `ResourceLimiter` that a growth failed *without
;; having asked it first*. A host that keeps any per-growth state across limiter
;; calls can be made to give that state back on demand from there, so the memory
;; this component wants is refused outright.
(component
  (core module $m
    (memory (export "m") i64 1)
  )
  (core instance (instantiate $m))
)
