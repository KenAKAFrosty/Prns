3 directives + 1 bonus: Hopspot, interfaces, control API; bonus: pure engine work


Hopspot has 4 roles: 
- An application we genuinely want to deliver (particularly on embedded, and especially on the Heltec (or any board with display, but that's the one we have currently) however; critically, it is *cross-platform* and we need to honor the other platforms as well)
- A nice visual diagnostics tool for *us*, as we build out Prns in general
- A proving ground for the consumer-side, high-level API of Prns. Dogfooding inherently.
- A real, practical example for app developers on how to integrate Prns into their app once all this is released.


For interfaces, we want to continue to expand real functionality on multi-platform, with genuine production-grade impls ready to go. 

Right now we're still tightening up the USB auto-interface, which is yet again a dogfooding & bootstrapping priority, like Hopspot as a whole is. That's because USB is one the simplest, cleanest ways to connect devices on the desk as we build. 

Following that trend, the next few I expect will be (not necessarily in this order):
- WiFi LAN auto-interface (parity), plus
    - An extension to that which we build
        - Properly uses a system to still connect even when there's a mesh AP situation (our BSSID issues from before)
        - Allows for when the host IS the AP (mobile hotspot functionality like on phones, but desktops these days do the same)
        - Allows for when you've connected to an AP that IS a Reticulum node host itself (the inverse of above)
- FIx and refine our USB auto-interface
    - It was the first built under the more clear multi-platform re-usable model. Overall it's a great start, but probably needs some refinement, both for API/organization, but even more so for reliability and performance as an actual target interface to use in the wild.
- Bluetooth, which is incredibly strong but has multiple things to handle:
    - Honor RNode format; this could be tricky but would be VERY powerful. Less important than the following two in terms of our long-term Bluetooth goals; however this is table stakes for release. So, what does that mean? It means we can skip it when doing focused bluetooth work if that helps, but we'll just have to come back to it
    - Honor Columba's format; check Sideband & MeshChat if they have their own
    - THen, our own that we can thoughtfully build
        - NOTE: THis is one of our most powerful cross-platform technology, so extra care for reliability, correctness, and performance is really really key here
- RNS parity for TCP/IP, both server and client
    - I suspect and hope these should be the lower lift ones; probably more straightforward. Good if we're feeling some pain and want a quick boost of momentum
    - We're already using smoltcp for embedded, so it should be even easier
- ESP-NOW
    - Possibly work to brand this system something internal, either Personal or Bramble related, but bramble is more awkward because it's not really cohesive as a "thing" yet
    - Most importantly, work on the perf side, make sure this is ROCK solid.
    - Nice thing is, this is only on embedded, so the scope is different from the others above
- LoRa vs GMSK @ 300kbps
    - We've done exploratory work in the past on using the sx1262's alternate mode for faster speed. 
    - Allowing for swapping here is the key piece: LoRa vs Speed mode (or whatever better name we give it)
    - Force-functions a lot of things: hot-changing interfaces, doing so from the Hopspot menu.
    - Related, just allowing for adjusting the LoRa settings from the Hopspot menu
    - This one is also Heltc-only; very scoped single target.

(Other interfaces will be needed, but aren't the high-priority, top-of-mind ones, for now)


Control API: this should feel unavoidable as we pursue the above two. Our Hopspot app will need ways to apply commands (I suspect a simple .ingest_commands() function on our EngineState, a sibling to .ingest_packets()), and also get events or updates or read-only functions to call, etc.  A simple starting example is the ability to announce (tying in to our aspirational/dummy menus on the Hopspot right now). It's a genuine API need, but also helps bootstrap our continued work once we have those tools available (Sensing a pattern here?).

This control API needs to be carefully designed; use all the forcing functions we have for this (top-lvel API, every platform has to pass; it should work despite sync/async, despite desktop vs mobile vs embedded). This is the API that will inform *every* consumer, including all of our target languages (see Principles & Ethos section about aspirational-website-as-target)


Pure engine work: 
There's still plenty more to go, but after we have some multi-platform devices running Hopspot with a few good interfaces, we'll be able to see and feel this work so much faster. However, we can use it, at any point in the process, as a relief valve on the pressure of doing the other three above. Those are unbounded problems we're trying to wrangle in, which can take a lot of focus and energy. Continuing reference-parity-matching work in our deterministic isolated no_std core should feel relatively sane and easy by comparison.




Principles & Ethos

- API design is paramount. This is one of the key reasons our work needs to stay iterative and tight: without my review, I find this part slips the fastest and hardest
- Make invalid states unrepresentable
- Newtypes, named enums, all incredibly powerful. We should not be shuffling around opaque bytes, or strings, or loose numbers, any more than we need to (like when the values being opaque *is the point*; that's a different thing)
- We're high-performance but FP-flavored, as the above implies
- Names are extremely important
- Comments should be an *exception*, not a rule. Many comments are papering over a bad name or bad API. Every comment should be first treated as a sign that the above principles were violated. If the nature of the comment doesn't apply to the above, then that's probably an indicator the comment is worthwhile.
- Use the Prns website as our working, malleable target. We are still in the early stages where it is not by any means complete, and we can adjust it as we need to. But otherwise, use it as a reference for "where we're going". 
