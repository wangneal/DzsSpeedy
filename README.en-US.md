<p align="center">
  <img src="https://github.com/user-attachments/assets/a82ceda2-9b7b-41e4-96dc-cd250c9bd3ff" width="120" />
</p>

<h1 align="center"> DzsSpeedy </h1>

<p align="center">
  The Best Open-Source Speed Accelerator for DZS (Dou Zhan Shen) · Game Speed Controller
</p>

<p align="center">
  <img src="https://api.visitorbadge.io/api/visitors?path=wangneal.dzsspeedy&countColor=%234ecdc4">
  <br/>
    
  <a href="https://github.com/wangneal/DzsSpeedy/stargazers">
    <img src="https://img.shields.io/github/stars/wangneal/DzsSpeedy?style=for-the-badge&color=yellow" alt="GitHub Stars">
  </a>

  <img src="https://img.shields.io/github/forks/wangneal/DzsSpeedy?style=for-the-badge&color=8a2be2" alt="GitHub Forks">

  <a href="https://github.com/wangneal/DzsSpeedy/issues">
    <img src="https://img.shields.io/github/issues-raw/wangneal/DzsSpeedy?style=for-the-badge&label=Issues&color=orange" alt="Github Issues">
  </a>
  <br/>  
  
  <a href="https://github.com/wangneal/DzsSpeedy/releases">
    <img src="https://img.shields.io/github/downloads/wangneal/DzsSpeedy/total?style=for-the-badge" alt="Downloads">
  </a>
  <a href="https://github.com/wangneal/DzsSpeedy/releases">
    <img src="https://img.shields.io/github/v/release/wangneal/DzsSpeedy?style=for-the-badge&color=brightgreen" alt="Version">
  </a>
  <a href="https://github.com/wangneal/DzsSpeedy/actions">
      <img src="https://img.shields.io/github/actions/workflow/status/wangneal/DzsSpeedy/build.yml?style=for-the-badge" alt="Github Action">
  </a>
  <a href="https://github.com/wangneal/DzsSpeedy">
    <img src="https://img.shields.io/badge/Platform-Windows-lightblue?style=for-the-badge" alt="Platform">
  </a>
  <br/>
  
  <a href="https://github.com/wangneal/DzsSpeedy/commits">
    <img src="https://img.shields.io/github/commit-activity/m/wangneal/DzsSpeedy?style=for-the-badge" alt="Commit Activity">
  </a>
  <img src="https://img.shields.io/badge/language-C/C++-blue?style=for-the-badge">
  <img src="https://img.shields.io/badge/License-GPLv3-green.svg?style=for-the-badge">
  <br/>
</p>

<p align="center">
  🌐 <a href="https://github.com/wangneal/DzsSpeedy/blob/master/README.en-US.md">English</a> |
  <a href="https://github.com/wangneal/DzsSpeedy/blob/master/README.de-DE.md">Deutsch</a> |
  <a href="https://github.com/wangneal/DzsSpeedy/blob/master/README.fr-FR.md">Français</a> |
  <a href="https://github.com/wangneal/DzsSpeedy/blob/master/README.ja-JP.md">日本語</a> |
  <a href="https://github.com/wangneal/DzsSpeedy/blob/master/README.ko-KR.md">한국어</a> |
  <a href="https://github.com/wangneal/DzsSpeedy/blob/master/README.md">中文</a>
</p>


# 🚀 Features
- Quick speed adjustment
- Modern UI
- Supports both x86 and x64 platform processes
- No kernel intrusion — Ring-3 level hooking, does not tamper with the system kernel


# 💾 Installation
📦 **Method 1: Winget**

``` powershell
# Install command
winget install dzsspeedy

# Open a new terminal and run dzsspeedy
dzsspeedy
```

📥 **Method 2: Manual Download**

Visit the [Releases page](https://github.com/wangneal/DzsSpeedy/releases) to download the latest version.


# 💻 System Requirements
- OS: Windows 10 or later
- Platform: x86 (32-bit) and x64 (64-bit)


# 📝 Usage
1. Launch DzsSpeedy
2. Run the target game (DZS · Dou Zhan Shen) you want to speed up
<img src="public/dzs-bg.png" width="50%">

3. Select the game process and adjust the speed multiplier in the DzsSpeedy interface
<img src="https://github.com/user-attachments/assets/9cd56353-1906-44c5-ba29-b5b4d2db2b80" width="50%"/>

4. Takes effect immediately — see the comparison below

<video src="https://github.com/user-attachments/assets/7c75e37d-bc7a-4639-89a0-a34a21676cba" width="70%"></video>

# 🔧 How It Works

Prerequisites:
- [Node.js](https://nodejs.org/) 18+
- [Rust](https://www.rust-lang.org/)
- [CMake](https://cmake.org/)
- [Visual Studio](https://visualstudio.microsoft.com/) (with C++ desktop development workload)

Build command:

``` powershell
npm run tauri dev
```

DzsSpeedy adjusts game speed by hooking the following Windows system time functions:

| Function | Library | Purpose |
|----------|---------|---------|
| Sleep | user32.dll | Thread sleep |
| SetTimer | user32.dll | Creates message-based timers |
| timeGetTime | winmm.dll | Retrieves system uptime in milliseconds |
| GetTickCount | kernel32.dll | Retrieves system uptime in milliseconds |
| GetTickCount64 | kernel32.dll | Retrieves system uptime in milliseconds (64-bit) |
| QueryPerformanceCounter | kernel32.dll | High-resolution performance counter |
| GetSystemTimeAsFileTime | kernel32.dll | Retrieves system time |
| GetSystemTimePreciseAsFileTime | kernel32.dll | Retrieves high-precision system time |
| SetWaitableTimer | kernel32.dll | Sets a waitable timer |
| SetWaitableTimerEx | kernel32.dll | Sets a waitable timer (extended) |

# ⚠️ Warnings
- This tool is for educational and research purposes only
- Some online games have anti-cheat systems — using this tool may result in account bans
- Excessive speed may cause game physics engine glitches or crashes
- Not recommended for use in competitive online games
- Open-source software without digital signatures may trigger false positives from antivirus software

# 🔄 Feedback
If you encounter any issues, please reach out via:
- [FAQ](https://github.com/wangneal/DzsSpeedy/wiki#faq) — Check the wiki first for common issues
- [GitHub Issues](https://github.com/wangneal/DzsSpeedy/issues) — Submit bug reports. Please do not submit cloud storage related issues, thank you for your cooperation~ 🙏


# 📜 License
DzsSpeedy is licensed under the GPL v3 license.

# 🙏 Acknowledgments
DzsSpeedy uses source code from the following projects. Thanks to the open-source community! If DzsSpeedy helps you, a Star is welcome!
- [minhook](https://github.com/TsudaKageyu/minhook): For API hooking
- [tauri](https://tauri.app/): GUI framework
- [MUI](https://mui.com/): UI component library
- [Ant Design](https://ant.design/): UI splitter component

Disclaimer: DzsSpeedy is intended for educational and research purposes only. Users assume all risks and liabilities associated with the use of this software. The author is not responsible for any loss or legal liability arising from the use of this software.

<a href="https://openomy.com/wangneal/dzsspeedy" target="_blank" style="display: block; width: 100%;" align="center">
  <img src="https://openomy.com/svg?repo=wangneal/dzsspeedy&chart=bubble&latestMonth=6" target="_blank" alt="Contribution Leaderboard" style="display: block; width: 100%;" />
</a>


<p align="center">
  <img src="https://api.star-history.com/svg?repos=wangneal/dzsspeedy&type=Date" Alt="Star History Chart">
</p>
