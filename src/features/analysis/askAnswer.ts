const circledNumbers = '①②③④⑤⑥⑦⑧⑨⑩⑪⑫⑬⑭⑮⑯⑰⑱⑲⑳'
const numberedItem = /^\d{1,2}[.、．]\s/
const cnSection = /^[一二三四五六七八九十]、\s*(.+)$/

/** Turn circled numbers, "1. 2. 3." and "一、二、" into Markdown blocks. */
export function formatAskAnswer(answer: string): string {
  let text = answer.replace(/\r\n?/g, '\n')
  for (let index = 0; index < circledNumbers.length; index += 1) {
    text = text.split(circledNumbers[index]).join(`\n${index + 1}. `)
  }
  text = text.replace(/[ \t]+(?=\d{1,2}[.、．]\s)/g, '\n')
  const lines = text.split('\n').map(line => line.trim()).filter(Boolean)
  const formatted: string[] = []
  let seenItem = false
  for (const line of lines) {
    const section = line.match(cnSection)
    if (section) {
      const [title, ...details] = section[1].split(' - ').map(part => part.trim()).filter(Boolean)
      if (formatted.length && formatted[formatted.length - 1] !== '') formatted.push('')
      seenItem = true
      formatted.push(`## ${title}`)
      if (details.length) {
        formatted.push('')
        formatted.push(...details.map(part => `- ${part}`))
      }
      continue
    }
    const item = numberedItem.test(line)
    if (item && formatted.length && !seenItem) formatted.push('')
    seenItem = seenItem || item
    formatted.push(line)
  }
  return formatted.join('\n').trim()
}
