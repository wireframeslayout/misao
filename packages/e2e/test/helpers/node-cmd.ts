/** TypeScript のソースを直接実行する node の argv (tsx + @misao/source 条件)。子プロセスにも同じ解決を使う。 */
export function nodeCmd(file: string, ...args: string[]): string[] {
  return [process.execPath, '--conditions=@misao/source', '--import', import.meta.resolve('tsx'), file, ...args];
}
