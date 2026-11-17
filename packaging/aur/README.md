# Publishing `antigravity-usage-monitor-git` to the AUR

AUR packages live in separate Git repositories hosted at `aur.archlinux.org`.
You can publish directly from NixOS (or Arch) once you have an AUR account and SSH key registered.

## 1. One-time AUR Account & SSH Key Setup

1. Create an account at <https://aur.archlinux.org/register>.
2. Copy your SSH public key:
   ```bash
   cat ~/.ssh/id_ed25519.pub
   ```
3. Paste it into **My Account → SSH Public Key** on `aur.archlinux.org`.
4. Add an AUR SSH host entry to `~/.ssh/config`:
   ```sshconfig
   Host aur.archlinux.org
     IdentityFile ~/.ssh/id_ed25519
     User aur
   ```

## 2. Clone the AUR Package Repo & Push

```bash
git clone ssh://aur@aur.archlinux.org/antigravity-usage-monitor-git.git /tmp/aur-agmonitor
cd /tmp/aur-agmonitor

cp /path/to/antigravity-usage-monitor/packaging/aur/PKGBUILD .
cp /path/to/antigravity-usage-monitor/packaging/aur/.SRCINFO .

# (If running on Arch Linux, you can also verify & regenerate .SRCINFO cleanly:)
# makepkg --printsrcinfo > .SRCINFO
# makepkg -si

git add PKGBUILD .SRCINFO
git commit -m "Initial import: antigravity-usage-monitor-git"
git branch -M master
git push -u origin master
```

## 3. Installing from AUR (Once Published)

Your Arch friends can then install with a single command:

```bash
yay -S antigravity-usage-monitor-git
# or
paru -S antigravity-usage-monitor-git
```
