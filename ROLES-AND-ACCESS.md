# Roles and access — a simple guide

Devboule has two kinds of user:

- **Admin** (the owner): runs everything, decides who gets in.
- **Collaborator**: works in the app (projects, agents, cloud, Oracle) but does not manage the team.

The app is the **same for everyone**. A signed "pass" decides who you are.

**In one sentence:** you sign a pass for each collaborator; their app checks your signature and opens in the right role. Only you can sign, because the signing key lives **only on your machine**.

---

## PART 1 — For YOU (admin), one time only

Do this **before** giving the app to collaborators:

1. Open the app and unlock.
2. Go to **Devices**.
3. Click **"Set this device as admin"**.
   → this puts your *public signature* inside the app.
4. Build the installer and give **that** build to collaborators.

Done. From now on that app trusts the passes you sign.

> Your **private** key (the one that signs) stays **only on your computer**. It never goes into the collaborators' app — that's why nobody else can sign passes.

---

## PART 2 — Adding a collaborator

### They do (on their machine)

1. Install the app and unlock.
2. The welcome screen appears → click **"Create device identity"**.
3. Click **"Copy join request"** and send you that text (chat/email).
   → it's only **public** info, safe to send.

### You do (on your machine)

1. Go to **Devices → Approve Invite**.
2. Paste their request and type their name.
3. Pick the **role**: *Collaborator* or *Admin*.
4. Click **"Approve device"**.
5. On their card, click **"Issue grant"** → copy the pass → send it back to them.

> ⚠️ **Before approving**, call them and check the *fingerprint* (the code you both see) matches. That stops anyone from impersonating them.

### They finish

1. Back on the welcome screen, **paste the pass**.
2. Click **"Verify & continue"** → they're in, with their role. 🎉

---

## Quick questions

**What does a collaborator see?**
Everything except the **Devices** page (team management). They work normally on Cloudflare/Scaleway — their agents run there.

**How do I limit what they can do in the cloud?**
Give them a **scoped cloud token** (fewer permissions). The limit is enforced by the provider (Cloudflare/Scaleway), not by the app.

**They changed computer or reinstalled?**
Repeat Part 2: new identity → new pass. (And remove the old device with **Revoke**.)

**Stuck in onboarding?**
The welcome screen has a **"Continue with limited access"** link: they get in anyway, with minimal permissions.

**Can I test roles without real collaborators?**
Yes, but only in the development build: a *role: admin / collaborator* switcher sits at the top (it does not exist in the distributed build).
