/**
 * 复制到剪贴板。**返回真的复制成功了没有。**
 *
 * 为什么要有这个文件（I2 自审发现）：原来两处「复制」按钮写的都是
 *
 * ```ts
 * void navigator.clipboard?.writeText(text)
 * setCopied(true)          // ← 不管成没成，一律显示「已复制」
 * ```
 *
 * `navigator.clipboard` 只在**安全上下文**里存在。Tauri 在 Linux 上走的是
 * `tauri://localhost` 自定义协议，WebKitGTK 认不认它是安全上下文要看版本；
 * 认不出来时 `navigator.clipboard` 直接是 `undefined`，那句可选链于是什么都没做，
 * 而界面照样跳出「已复制」—— 用户切到终端一粘，空的。
 *
 * 这正是红线 1 说的「不许假装成功」。所以这里：
 *
 * 1. 先试 `navigator.clipboard.writeText`（有就用，它是唯一不需要焦点技巧的路径）；
 * 2. 不行就退回 `document.execCommand('copy')` ——  老、被标记为废弃，但在
 *    WebKitGTK 里一直是能用的，而且不要求安全上下文；
 * 3. 两条都不行就**如实返回 false**，由调用方显示「复制不了，请手动选中下面这行」。
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    /* 落到下面那条路 */
  }
  return legacyCopy(text)
}

/** `document.execCommand('copy')` 那条老路。选中一个临时 textarea 再执行复制。 */
function legacyCopy(text: string): boolean {
  try {
    const ta = document.createElement('textarea')
    ta.value = text
    // 不能用 display:none / visibility:hidden —— 那样选不中，execCommand 会是 false
    ta.setAttribute('readonly', '')
    ta.style.position = 'fixed'
    ta.style.top = '-1000px'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    ta.setSelectionRange(0, text.length)
    const ok = document.execCommand('copy')
    document.body.removeChild(ta)
    return ok
  } catch {
    return false
  }
}
