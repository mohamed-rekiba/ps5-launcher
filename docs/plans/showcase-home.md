# Plan: the Showcase home (the default look, inspired by ARTFLIX)

Status: **approved**. The owner asked for a default theme like
[Alekfull-ARTFLIX](https://github.com/fagnerpc/Alekfull-ARTFLIX); the advisor (Codex) decided how.
Reference shots of ARTFLIX are not stored here (its art is licensed for non-commercial use only).

My decision: build **Showcase**, an original ARTFLIX-inspired compiled home layout, and make it the default. Keep the current layout selectable as **Classic**.

**1. Licensing**

Do not import ARTFLIX artwork, logos, sounds, XML, or bundled fonts into our releases.

The supplied README terms permit some noncommercial sharing and modification, with attribution, change notices, and the same license. They do not grant unrestricted redistribution. No SPDX identifier does not mean no license; equally, “CC BY-NC-SA style” is not enough to identify an exact Creative Commons license. Shipping through deb/rpm/AppImage or an OS image does not remove those conditions.

Reimplement the broad visual ideas in original Slint code: hero background, readable gradient, information column, red actions, and vertical selector. Avoid tracing the author’s artwork or recreating the exact branded composition. Copyright distinguishes ideas and systems from their protected expression. [U.S. Copyright Office](https://www.copyright.gov/help/faq/faq-protect.html)

Use our existing fetched game art under its existing permissions. Fetching does not itself grant redistribution rights; do not bake that cache into release images.

For this independent implementation, ARTFLIX attribution is not a license condition. Add a voluntary About credit: “Showcase’s visual direction was inspired by Alekfull-ARTFLIX by fagnerpc,” linked to the project. Use our own wordmark. Separately, adopt a project license before release; our missing LICENSE is unresolved.

**2. Architecture**

Add a compiled `ShowcaseHome` beside `ClassicHome`. Both consume the same selected-game data and action interface. Keep selection, launch, installation, downloads, and media handling shared.

Settings → Appearance gets **Home layout: Showcase / Classic**. Initially, compiled defaults select Showcase. Later, the default theme declares Showcase as its preferred layout. An explicit user layout override wins.

Change **Home only** structurally. Keep Library’s grid, filters, and search; Game Hub; settings; downloads; dialogs; and power screens. Apply shared colours and fonts across them. Themes remain colours/fonts/backgrounds/icons/sounds plus supported layout choices, as [the plan specifies](/Users/mohamedshabaan/temp/ps5-launcher/docs/plans/addons.md:56).

**3. Controller UX**

Use a 1920×1080 reference canvas, scaled proportionally with a 64px safe margin.

- **Background:** full-bleed hero art, positioned to preserve the subject. Strong dark gradient across the left 800px; lighter dark backing beneath the right wheel. Missing art uses a built-in abstract background.
- **Header, y=40–112:** our condensed wordmark at x=64, then Home / Library. Top-right: search, settings, power, storage/downloads, controllers. Move the clock to bottom-right.
- **Information, x=64–744, y=170–510:** game logo within 680×150; title fallback at 64px. Below: `year | developer`, using publisher only when developer is unavailable. Then genre, installed/download state, and size. Description: four lines at 26px, truncated; full text remains in Game Hub. Omit missing fields.
- **Actions, y=535–603:** primary **Play Now** when installed; **Install** when available; **Downloads · N%** while downloading/installing. Failures lead to download details. Secondary is always outlined **Game Hub**, never a fabricated “3 games.”
- **Preview, x=64, y=650, 640×360:** cycle screenshots every eight seconds after selection settles. No trailer autoplay. Show the first screenshot immediately; fallback to cover art. Rating sits just above it, with stars and the supplied numeric scale. Missing rating is omitted.
- **Wheel, x=1456–1856, y=150–960:** five game logos, selected centred and larger. Neighbours dimmed but readable. Missing logos become text titles. Show position/count. Reuse Home’s existing game set and ordering.
- **Footer, y=1032:** contextual controller hints, with clock at the right.

Focus starts on the remembered wheel selection. Up/down selects previous/next game, without wrapping. Left enters the primary action; right from either action returns to the wheel. Left/right traverses actions before exiting. A on the wheel enters actions; A on a button executes it. B returns to the wheel.

LB/RB switches Home/Library. Y enters the header; left/right traverses its controls, down restores prior content focus. Storage opens Downloads; controllers opens controller status. Clock and preview are informational. Focus uses a white outline and slight enlargement, independent of red.

With no games, show “Your library is empty,” focus **Open Library**, and retain settings access.

Reduced motion disables wheel movement, crossfades, and automatic screenshot cycling.

Use accent **#E50914**, white text, and charcoal panels. Use **Barlow Condensed Bold** for our wordmark/headings and **Inter** for body text. Both use **SIL OFL 1.1**; ship their copyright notices and licenses in every package/image. [Barlow license](https://raw.githubusercontent.com/google/fonts/main/ofl/barlowcondensed/OFL.txt), [Inter license](https://raw.githubusercontent.com/google/fonts/main/ofl/inter/OFL.txt)

**4. Order**

Do this now as an independent UI task. Implement shared home data, Showcase, navigation, and the persisted layout choice. Verify controller paths, missing metadata, empty state, downloads, and reduced motion.

Keep the filesystem theme loader after the emulator phases. Later, connect its defaults and tokens to these compiled layouts.