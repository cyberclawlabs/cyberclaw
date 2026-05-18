// 极简 i18n —— 只覆盖 v2 skeleton 即时需要的 key。
// 老 stack 的完整 STRINGS（~3000 行）在 F5 迁完时再合并过来；
// 本文件目前只承担 "v2 stack 不再硬编码英文 + 双语可切" 的最小契约。

export type Lang = "en" | "zh-CN";

const STRINGS: Record<Lang, Record<string, string>> = {
  en: {
    "app.brand": "CyberClaw",
    "app.subtitle": "Controlled agent platform",
    "auth.login.title": "Sign in",
    "auth.login.user_id": "User ID",
    "auth.login.password": "Password",
    "auth.login.submit": "Sign in",
    "auth.login.failed": "Sign-in failed",
    "auth.logout": "Sign out",
    "common.loading": "Loading…",
    "common.error": "Error",
    "common.retry": "Retry",
    "footer.version": "Version",
    "footer.commit": "Commit",
    "footer.build_time": "Built",
    "footer.role": "Role",
    "chat.send": "Send",
    "chat.stop": "Stop",
    "chat.new": "New conversation",
    "chat.empty": "Pick or start a conversation",
    "chat.placeholder": "Ask anything…  (↵ send · ⇧↵ newline)",
  },
  "zh-CN": {
    "app.brand": "CyberClaw",
    "app.subtitle": "受控智能体平台",
    "auth.login.title": "登录",
    "auth.login.user_id": "用户 ID",
    "auth.login.password": "密码",
    "auth.login.submit": "登录",
    "auth.login.failed": "登录失败",
    "auth.logout": "退出",
    "common.loading": "加载中…",
    "common.error": "出错",
    "common.retry": "重试",
    "footer.version": "版本",
    "footer.commit": "提交",
    "footer.build_time": "构建于",
    "footer.role": "角色",
    "chat.send": "发送",
    "chat.stop": "停止",
    "chat.new": "新会话",
    "chat.empty": "选择或新建一个会话",
    "chat.placeholder": "问点什么… (↵ 发送 · ⇧↵ 换行)",
  },
};

const LANG_KEY = "cyberclaw.admin.lang";

export function getLang(): Lang {
  try {
    const v = localStorage.getItem(LANG_KEY);
    if (v === "en" || v === "zh-CN") return v;
  } catch {}
  return navigator.language?.startsWith("zh") ? "zh-CN" : "en";
}

export function setLang(lang: Lang): void {
  try {
    localStorage.setItem(LANG_KEY, lang);
  } catch {}
}

export function t(key: string, lang: Lang = getLang()): string {
  return STRINGS[lang]?.[key] ?? STRINGS.en[key] ?? key;
}
