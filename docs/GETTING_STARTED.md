# Getting started with Chirper

Chirper shows the chats of several networks in one inbox. Each account signs in with that network's
own login. Nothing goes through a Chirper server.

---

## 1. Install

Release packages are on the [GitHub Releases](https://github.com/itsrhotaudevvy/chirper/releases)
page. The release workflow produces:

- **macOS:** a `.dmg` and an `.app`, universal, Apple silicon, or Intel.
- **Windows:** an `.msi` and an NSIS `.exe`, for x64, x86 and arm64.
- **Linux:** an `.AppImage`, a `.deb` and an `.rpm`, for x64 and arm64.
- **Android:** APKs. A universal APK and one APK per ABI. The release is signed only when the
  signing secrets are set in the build; otherwise the APK is unsigned and must be signed before it
  is installed. To install a debug or unsigned APK from a computer, use `adb install -r <file>.apk`.
- **iOS:** builds are produced in CI for App Store Connect and the simulator. This repository does
  not document a public iOS distribution path.

On Linux there is no app lock and no screen-capture exclusion (see section 5).

---

## 2. First run

- On desktop, the app creates a random 256-bit database key on first start and stores it in the OS
  credential store (macOS Keychain, Windows Credential Manager, or the Secret Service on Linux). You
  are not asked for a passphrase.
- On Android and iOS the key is wrapped by the Android Keystore or kept in the iOS Keychain.
- Onboarding then leads to the inbox. With no accounts the inbox is empty.

The database is encrypted with that key. A copy of `chirper.db` alone cannot be opened on another
device, because the key stays in this device's keystore. Section 4 of
[SECURITY_MODEL.md](SECURITY_MODEL.md) describes what happens when the database cannot be opened.

---

## 3. Adding an account

1. Open the **Accounts** section of Settings and choose **Add network**. A network that is not usable
   is shown dimmed; tapping it shows its reason and does not start a login.
2. Pick a network. Its login flows are listed with a short description. The flows are:

   | Network | Flows |
   |---|---|
   | Discord | Account token, or QR code (scan with the Discord mobile app) |
   | Fluxer | Session token, bot token, or e-mail or username and password |
   | Telegram (in builds with the API id) | Phone number with login code and two-step password, QR code, or bot token |
   | Slack | E-mail (with a code), an auth token with a cookie, or an app token |
   | Bluesky | Handle and an app password |
   | LinkedIn | Browser cookies: sign in on linkedin.com, and the app reads the session cookies |
   | Google Voice | Browser cookies: sign in on voice.google.com, and the app reads the session cookies |
   | SMS (Android) | This phone: the app asks for SMS permission |

   The full list, with the capabilities of each network, is in [NETWORKS.md](NETWORKS.md).
3. For a QR code, scan it with the network's app as the screen describes. For a cookie flow, the
   sign-in page opens in a window inside Chirper and the app reads the listed cookies once you are
   signed in.
4. When the account is connected, the app syncs the recent chats in the background. **Settings →
   Accounts** shows each account's state. **Sync history** and **Log out** are on the account's page.

---

## 4. The inbox

- Each network has its own colour, shown on chats, accounts and the network strip.
- **Cmd/Ctrl + K** opens search.
- **Cmd/Ctrl + 1 to 9** opens the pinned chat in that position.
- In the inbox, the arrow keys move between chats and **Enter** opens the chat under the cursor.
- **Escape** returns to the inbox from a chat.

In a chat:

- **Enter** sends; **Shift + Enter** adds a line.
- **Up arrow** in an empty composer edits your last message.
- **Escape** cancels a reply or an edit.

The account page shows the read-receipt and typing options only for networks that support them. For
the other actions, a command the network does not support fails with an error from the runtime.

---

## 5. Privacy and locking

Settings has these privacy controls:

- **App lock** (Privacy section of Settings): asks for the platform's authentication when the app comes
  back after **Lock after** seconds in the background. Available on macOS (Touch ID or password),
  Windows (Windows Hello), Android (biometrics) and iOS (Face ID or Touch ID). Not available on Linux.
- **Block screenshots**: asks the system to exclude Chirper's window from screenshots and screen
  sharing. It is on by default on Android and iOS and off on desktop. On Linux it is not available.
- **Notifications** and **Hide message text**: turn notifications on or off, and keep the message
  text out of them.
- **Haptics** turns touch feedback on or off.

Chirper stores its data in the application data directory: the encrypted database `chirper.db`, the
sealed media in `media/`, the sealed per-login secrets in `credentials/`, and the plain settings files
`settings.json` and `prefs.json`. [PRIVACY.md](../PRIVACY.md) lists what is stored where.

**Settings → Sync** makes a bundle of settings, the account list and chat pin, archive and mute
state, encrypted with a passphrase or PIN you choose. The bundle holds no login credentials, so each
account still has to sign in again on the other device.

---

