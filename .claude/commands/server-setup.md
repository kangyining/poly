# /server-setup

Audit and apply Ubuntu stability settings for a machine that needs to run 24/7 (trading bots, recorders, local servers).

## Step 1 — Ask if desktop or laptop

If not already known, ask the user before proceeding. The answer determines whether lid-close handling applies.

## Step 2 — Audit current state

Run these checks in a single bash command and report findings in a clear table (Applied / Not applied / N/A):

```bash
echo "=== Sleep targets ===" && systemctl is-enabled sleep.target suspend.target hibernate.target hybrid-sleep.target 2>&1
echo "=== Lid close (laptop only) ===" && grep -E "HandleLidSwitch|HandleSuspendKey" /etc/systemd/logind.conf 2>&1
echo "=== Swap ===" && swapon --show
echo "=== kernel.panic ===" && sysctl kernel.panic 2>&1
echo "=== apt timers ===" && systemctl is-enabled apt-daily.timer apt-daily-upgrade.timer 2>&1
```

## Step 3 — Apply only what is missing

Work through each item below. Check before applying — never re-apply something already in place.

### 1. Mask sleep/suspend/hibernate (all machines)

**Check:** sleep.target should be `masked`
**Apply:**
```bash
sudo systemctl mask sleep.target suspend.target hibernate.target hybrid-sleep.target
```
**Revert:**
```bash
sudo systemctl unmask sleep.target suspend.target hibernate.target hybrid-sleep.target
```

---

### 2. Lid close ignore (laptop only — skip on desktop)

**Check:** `/etc/systemd/logind.conf` should have `HandleLidSwitch=ignore`
**Apply:** Add to `/etc/systemd/logind.conf`:
```
HandleLidSwitch=ignore
HandleLidSwitchExternalPower=ignore
HandleSuspendKey=ignore
```
Then: `sudo systemctl restart systemd-logind`

**Revert:** Set back to `suspend` and restart systemd-logind.

---

### 3. Swap (skip if swap already exists)

**Check:** `swapon --show` should show an active swap
**Apply only if no swap exists:**
```bash
sudo fallocate -l 4G /swapfile
sudo chmod 600 /swapfile
sudo mkswap /swapfile
sudo swapon /swapfile
echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab
```
**Revert:**
```bash
sudo swapoff /swapfile && sudo rm /swapfile
# Remove /swapfile line from /etc/fstab
```

---

### 4. Kernel panic auto-reboot (all machines)

**Check:** `sysctl kernel.panic` should return `10`
**Apply:** Append to `/etc/sysctl.conf` and activate:
```bash
echo 'kernel.panic=10' | sudo tee -a /etc/sysctl.conf
sudo sysctl -p
```
**Note:** Do NOT set `kernel.panic_on_oops=1` — too aggressive, causes unnecessary reboots.

**Revert:** Remove `kernel.panic=10` from `/etc/sysctl.conf`, then `sudo sysctl -p`

---

### 5. Disable automatic updates (all machines)

**What:** Stops apt-daily.timer (package list downloads) and apt-daily-upgrade.timer (automatic installs). Prevents disk I/O spikes, CPU spikes from dpkg post-install scripts, and apt lock contention. Critical for HFT, important for any always-on bot.

**Note:** User must run updates manually: `sudo apt update && sudo apt upgrade`

**Check:** Both timers should be `disabled`
**Apply:**
```bash
sudo systemctl disable --now apt-daily.timer apt-daily-upgrade.timer
```
**Revert:**
```bash
sudo systemctl enable --now apt-daily.timer apt-daily-upgrade.timer
```

---

## Step 4 — Verify

After applying, re-run the audit checks and confirm everything shows the correct state.

## What to skip and why

| Setting | Reason to skip |
|---------|---------------|
| Screen blanking | Cosmetic — blank screen doesn't affect background processes |
| Unattended auto-reboot | Already off by default on Ubuntu |
| Reduce swappiness | Only relevant under memory pressure; lightweight bots won't hit this |
| NetworkManager autoconnect-retries | Already default NM behaviour; the setting only affects boot-time retries |
| DNS change | Only change if the user reports DNS reliability issues |
| kernel.panic_on_oops | Turns minor kernel warnings into reboots — too aggressive |
| systemd watchdog | Guards against systemd hanging, extremely rare on stable Ubuntu |
