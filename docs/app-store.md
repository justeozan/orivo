# App Store

What the App Store Connect record for Orivo declares, and why. The answers are
derived from the code, not from intent: if what the app sends off the device
changes, this file, [`PRIVACY.md`](../PRIVACY.md) and the record have to move
together.

| | |
| --- | --- |
| Record | **Orivo Games**, Apple ID `6813145265`, bundle ID `run.evey.orivo` |
| Versions | iOS 1.0 and macOS 1.0 |
| Devices | iPhone and iPad (`TARGETED_DEVICE_FAMILY = "1,2"`) |
| Listing language | English (U.S.) |
| Privacy policy | <https://github.com/justeozan/orivo/blob/main/PRIVACY.md> |
| Support | <https://github.com/justeozan/orivo/issues> |
| Release | manual, after approval |

## Listing

- **Subtitle**: All your games in one place
- **Categories**: Games, then Entertainment
- **Keywords**: no store or game names. Third-party brands in keywords are
  rejected under guideline 2.3.7; the description names the stores instead.
- **Content rights**: the app shows third-party content (store artwork,
  descriptions and logos from the stores' public catalogs).
- **Price**: free. **Availability**: every territory except mainland China,
  where a game needs a licence and an approval number before it can be listed.

## Age rating: 12+

Orivo itself shows nothing objectionable, but every screen is built from the
art of the games in the library, and that art includes weapons and fantasy and
realistic violence. Those three, plus horror themes, are answered
*infrequent*. Everything else is *none* or *no*: no advertising, no gambling,
no loot boxes, no chat, no user-generated content, no health topics, no
unrestricted web access.

## App Privacy

There is no public API for this form: App Store Connect › App Privacy.

Everything hinges on one build setting. `src/sentry.ts` only starts Sentry
when `VITE_SENTRY_DSN` is set **when the frontend is built**, and the release
workflow sets it from a repository secret.

**Built without the DSN**: answer *Data Not Collected*.

**Built with the DSN**: *Data collected*, none of it used for tracking.

| Apple category | Type | Where it comes from | Purpose | Linked to the user |
| --- | --- | --- | --- | --- |
| Diagnostics | Crash Data | Sentry error reports | App Functionality | No |
| Diagnostics | Performance Data | Sentry traces, 10 % of sessions | App Functionality | No |
| Contact Info | Name | optional field of the Feedback form | App Functionality | Yes |
| Contact Info | Email Address | optional field of the Feedback form | App Functionality | Yes |
| User Content | Customer Support | the Feedback message and its optional screenshot | App Functionality | Yes |

`sendDefaultPii: false` keeps names and IP addresses off the error reports.

Not "collected" in Apple's sense:

- Store sign-ins (Steam, Epic Games, GOG, Ubisoft Connect, Xbox, Microsoft
  Store, Instant Gaming): the user signs in on the store's own page, and the
  credential stays in the device keychain. No Orivo server is involved.
- The library, preferences, wishlist and the IGDB / SteamGridDB / Google keys:
  stored on the device only.
- Requests to the stores' public catalogs and CDNs: no personal data is sent.

No advertising or analytics SDK, no advertising identifier.

## Review notes

The reviewer needs no store account: Settings › Appearance › *Beta & debug* ›
**Show demo games** fills the library with the bundled demo games. The notes
on the record say so, in English, and the review contact is filled in there.

## Screenshots

Six per device, all landscape: iPhone 6.9" (2868 × 1320), iPad 13"
(2752 × 2064) and Mac (2880 × 1800). They are the real frontend, captured at
Apple's sizes with a library built from the official Steam and IGDB art of
twelve games, so the pipeline lives outside this repository with that art.

On iPhone and iPad no game is launchable and the one on stage comes from
Steam, because that is what the iOS build shows: a PC game cannot start on
iOS, so its button reads *Install*, not *Play*. Only the Mac screenshots show
*Play*. Screenshots have to match the app (guideline 2.3).
