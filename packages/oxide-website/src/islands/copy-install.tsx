import { useCallback, useEffect, useRef, useState } from 'react'
import { Copy, Check } from 'lucide-react'

type Status = 'idle' | 'copied' | 'failed'

interface Props {
  command: string
}

export default function CopyInstall({ command }: Props) {
  const [status, setStatus] = useState<Status>('idle')
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current)
  }, [])

  const onClick = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(command)
      setStatus('copied')
    } catch {
      setStatus('failed')
    }
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(() => setStatus('idle'), 2000)
  }, [command])

  return (
    <div className="flex items-center justify-between w-full bg-[#09090b] border border-zinc-800 rounded-lg p-2 pl-4">

      {/* Command side with $ */}
      <div className="flex items-center gap-3 font-mono text-sm">
        <span className="text-[#e45631] select-none">$</span>
        <code className="text-zinc-300">{command}</code>
      </div>

      {/* Copy Button */}
      <button
        type="button"
        onClick={onClick}
        className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-mono text-zinc-400 bg-zinc-900 hover:bg-zinc-800 hover:text-zinc-200 rounded-md border border-zinc-800 hover:border-zinc-700 transition-colors shrink-0 ml-4"
        aria-label="Copy install command"
      >
        {status === 'idle' && (
          <>
            <Copy size={14} />
            <span>Copy</span>
          </>
        )}
        {status === 'copied' && (
          <>
            <Check size={14} className="text-green-500" />
            <span className="text-green-500">Copied</span>
          </>
        )}
        {status === 'failed' && (
          <>
            <span>Select</span>
          </>
        )}
      </button>
    </div>
  )
}
