# Avila Labs account (optional)

OpenBNCT can sign you in to an Avila Labs account. Signing in is optional, and every OpenBNCT feature works the same without it. OpenBNCT is research software, not a medical device, with or without an account.

## What signing in does

One sign-in works across ACTINV, Converra and OpenBNCT, so you sign in once and the tools recognise you. The account is a sign-in only. It does not change what OpenBNCT calculates.

## What is sent

Nothing from your work is sent. No inputs, results, images, structures, plans or files leave your machine because you signed in. Signing in exchanges only what is needed to identify your account.

## Browser viewer

The web viewer shows a **Sign in** button in the header and a prompt on your first visit. You can dismiss the prompt and carry on without an account.

## Desktop app

The desktop app uses a device sign-in. Choose **Sign in** in the header; the app shows a short code and opens the Avila Labs sign-in page in your browser. Approve the sign-in there and the app signs in.

The desktop token is stored in `~/.config/avila/credentials.json`. **Sign out** revokes the token and removes it from that file. Deleting the file signs you out on that machine without contacting anyone.

## Privacy and terms

- Privacy notice: <https://api.avilalabs.org/privacy>
- Terms: <https://api.avilalabs.org/terms>
