3 directives: Hopspot, interfaces, and final parity-chasing work;


Hopspot has 4 roles: 
- An application we genuinely want to deliver (particularly on embedded, and especially on the Heltec (or any board with display, but that's the one we have currently) however; critically, it is *cross-platform* and we need to honor the other platforms as well)
- A nice visual diagnostics tool for *us*, as we build out Prns in general
- A proving ground for the consumer-side, high-level API of Prns. Dogfooding inherently.
- A real, practical example for app developers on how to integrate Prns into their app once all this is released.


For interfaces, we want to continue to expand real functionality on multi-platform, with genuine production-grade impls ready to go. As of now that's all refinement work. Correct, robust, fast. That's the mantra to be internalized. Currently we're still refining the auto powerhouses, particularly BLE. 


Principles & Ethos

- API design is paramount. This is one of the key reasons our work needs to stay iterative and tight: without my review, I find this part slips the fastest and hardest
- Make invalid states unrepresentable
- Newtypes, named enums, all incredibly powerful. We should not be shuffling around opaque bytes, or strings, or loose numbers, any more than we need to (like when the values being opaque *is the point*; that's a different thing)
- We're high-performance but FP-flavored, as the above implies
- Names are extremely important
- Encode principles structurally
- Comments should be an *exception*, not a rule. Many comments are papering over a bad name or bad API. Every comment should be first treated as a sign that the above principles were violated. If the nature of the comment doesn't apply to the above, then that's probably an indicator the comment is worthwhile.
- Use the Prns website as our working, malleable target. We are still in the early stages where it is not by any means complete, and we can adjust it as we need to. But otherwise, use it as a reference for "where we're going". 
