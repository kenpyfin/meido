/**
 * Local thread UI primitives (replaces @assistant-ui/react-ui).
 * Keeps `.aui-*` class hooks used by styles.css.
 */
import React, { forwardRef, memo, useEffect, useMemo, useState, type ComponentType, type FC, type PropsWithChildren, type ReactNode } from 'react'
import {
  AssistantRuntimeProvider,
  AttachmentPrimitive,
  BranchPickerPrimitive,
  ComposerPrimitive,
  MessagePartPrimitive,
  MessagePrimitive,
  ThreadPrimitive,
  useAssistantRuntime,
  useAttachment,
  useThread,
  type AssistantRuntime,
  type AssistantToolUI,
  type EmptyMessagePartComponent,
  type TextMessagePartComponent,
  type ToolCallMessagePartProps,
} from '@assistant-ui/react'
import {
  MarkdownTextPrimitive,
  unstable_memoizeMarkdownComponents,
  useIsMarkdownCodeBlock,
  type MarkdownTextPrimitiveProps,
} from '@assistant-ui/react-markdown'
import { INTERNAL } from '@assistant-ui/react'
import {
  CheckIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  CircleXIcon,
  CopyIcon,
  FileIcon,
  PaperclipIcon,
  SendHorizontalIcon,
  SquareIcon,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from '@/components/ui/dialog'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { copyTextToClipboard } from '@/lib/copy-to-clipboard'

const { withSmoothContextProvider, useSmoothStatus } = INTERNAL

// --- thread config (from react-ui thread-config) ---

export type SuggestionConfig = {
  text?: ReactNode | undefined
  prompt: string
}

export type ThreadWelcomeConfig = {
  message?: string | null | undefined
  suggestions?: SuggestionConfig[] | undefined
}

export type UserMessageConfig = {
  allowEdit?: boolean | undefined
}

export type AssistantMessageConfig = {
  allowReload?: boolean | undefined
  allowCopy?: boolean | undefined
  allowSpeak?: boolean | undefined
  allowFeedbackPositive?: boolean | undefined
  allowFeedbackNegative?: boolean | undefined
  components?: {
    Text?: TextMessagePartComponent | undefined
    Empty?: EmptyMessagePartComponent | undefined
    ToolFallback?: ComponentType<ToolCallMessagePartProps> | undefined
    Footer?: ComponentType | undefined
  } | undefined
}

export type BranchPickerConfig = {
  allowBranchPicker?: boolean | undefined
}

export type ComposerConfig = {
  allowAttachments?: boolean | undefined
}

export type StringsConfig = {
  thread?: {
    scrollToBottom?: { tooltip?: string | undefined }
  }
  branchPicker?: {
    previous?: { tooltip?: string | undefined }
    next?: { tooltip?: string | undefined }
  }
  composer?: {
    send?: { tooltip?: string | undefined }
    cancel?: { tooltip?: string | undefined }
    addAttachment?: { tooltip?: string | undefined }
    removeAttachment?: { tooltip?: string | undefined }
    input?: { placeholder?: string | undefined }
  }
  editComposer?: {
    send?: { label?: string | undefined }
    cancel?: { label?: string | undefined }
  }
  code?: {
    header?: { copy?: { tooltip?: string | undefined } }
  }
}

export type ThreadConfig = {
  runtime?: AssistantRuntime | undefined
  welcome?: ThreadWelcomeConfig | undefined
  assistantMessage?: AssistantMessageConfig | undefined
  userMessage?: UserMessageConfig | undefined
  branchPicker?: BranchPickerConfig | undefined
  composer?: ComposerConfig | undefined
  strings?: StringsConfig | undefined
  tools?: AssistantToolUI[] | undefined
  components?: {
    UserMessage?: ComponentType | undefined
    AssistantMessage?: ComponentType | undefined
    EditComposer?: ComponentType | undefined
    Composer?: ComponentType | undefined
    ThreadWelcome?: ComponentType | undefined
    MessagesFooter?: ComponentType | undefined
  } | undefined
}

const ThreadConfigContext = React.createContext<ThreadConfig>({})

export const useThreadConfig = (): Omit<ThreadConfig, 'runtime'> => React.useContext(ThreadConfigContext)

export type ThreadConfigProviderProps = PropsWithChildren<{
  config?: ThreadConfig | undefined
}>

export const ThreadConfigProvider: FC<ThreadConfigProviderProps> = ({ children, config }) => {
  const hasAssistant = !!useAssistantRuntime({ optional: true })
  const outerConfig = useThreadConfig()
  const hasConfig = config && Object.keys(config).length > 0
  if (hasConfig && Object.keys(outerConfig).length > 0) {
    throw new Error(
      'You are providing ThreadConfig to several nested components. Please provide all configuration to the same component.',
    )
  }
  const configProvider = hasConfig ? (
    <ThreadConfigContext.Provider value={config}>{children}</ThreadConfigContext.Provider>
  ) : (
    <>{children}</>
  )
  if (!config?.runtime) return configProvider
  if (hasAssistant) {
    throw new Error(
      'You provided a runtime to <Thread> while simulataneously using <AssistantRuntimeProvider>. This is not allowed.',
    )
  }
  return <AssistantRuntimeProvider runtime={config.runtime}>{configProvider}</AssistantRuntimeProvider>
}

// --- withDefaults helper ---

function withDefaults<P extends { className?: string }>(
  Component: React.ElementType,
  defaultProps: P,
) {
  const { className: defaultClassName, ...restDefaults } = defaultProps
  return forwardRef<unknown, P>((props, ref) => {
    const { className, ...rest } = props
    return (
      <Component
        {...restDefaults}
        {...rest}
        className={cn(defaultClassName, className)}
        ref={ref as React.Ref<never>}
      />
    )
  })
}

// --- tooltip icon button ---

type TooltipIconButtonProps = React.ComponentProps<typeof Button> & {
  tooltip: string
  side?: 'top' | 'right' | 'bottom' | 'left'
}

const TooltipIconButton = forwardRef<HTMLButtonElement, TooltipIconButtonProps>(
  ({ children, tooltip, side = 'bottom', className, ...rest }, ref) => (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button variant="ghost" size="icon" className={className} {...rest} ref={ref}>
          {children}
          <span className="aui-sr-only">{tooltip}</span>
        </Button>
      </TooltipTrigger>
      <TooltipContent side={side}>{tooltip}</TooltipContent>
    </Tooltip>
  ),
)
TooltipIconButton.displayName = 'TooltipIconButton'

// --- markdown (legacy thread UI path) ---

function CodeHeader({ language, code }: { language?: string; code?: string }) {
  const {
    strings: { code: { header: { copy: { tooltip = 'Copy' } = {} } = {} } = {} } = {},
  } = useThreadConfig()
  const [copied, setCopied] = useState(false)
  const onCopy = () => {
    if (!code || copied) return
    void copyTextToClipboard(code).then((ok) => {
      if (!ok) return
      setCopied(true)
      window.setTimeout(() => setCopied(false), 2000)
    })
  }
  return (
    <div className="aui-code-header-root">
      <span className="aui-code-header-language">{language}</span>
      <TooltipIconButton tooltip={tooltip} onClick={onCopy} type="button">
        {!copied ? <CopyIcon /> : <CheckIcon />}
      </TooltipIconButton>
    </div>
  )
}

const defaultMarkdownComponents = unstable_memoizeMarkdownComponents({
  h1: ({ className, ...props }) => <h1 className={cn('aui-md-h1', className)} {...props} />,
  h2: ({ className, ...props }) => <h2 className={cn('aui-md-h2', className)} {...props} />,
  h3: ({ className, ...props }) => <h3 className={cn('aui-md-h3', className)} {...props} />,
  h4: ({ className, ...props }) => <h4 className={cn('aui-md-h4', className)} {...props} />,
  h5: ({ className, ...props }) => <h5 className={cn('aui-md-h5', className)} {...props} />,
  h6: ({ className, ...props }) => <h6 className={cn('aui-md-h6', className)} {...props} />,
  p: ({ className, ...props }) => <p className={cn('aui-md-p', className)} {...props} />,
  a: ({ className, ...props }) => <a className={cn('aui-md-a', className)} {...props} />,
  blockquote: ({ className, ...props }) => (
    <blockquote className={cn('aui-md-blockquote', className)} {...props} />
  ),
  ul: ({ className, ...props }) => <ul className={cn('aui-md-ul', className)} {...props} />,
  ol: ({ className, ...props }) => <ol className={cn('aui-md-ol', className)} {...props} />,
  hr: ({ className, ...props }) => <hr className={cn('aui-md-hr', className)} {...props} />,
  table: ({ className, ...props }) => <table className={cn('aui-md-table', className)} {...props} />,
  th: ({ className, ...props }) => <th className={cn('aui-md-th', className)} {...props} />,
  td: ({ className, ...props }) => <td className={cn('aui-md-td', className)} {...props} />,
  tr: ({ className, ...props }) => <tr className={cn('aui-md-tr', className)} {...props} />,
  sup: ({ className, ...props }) => <sup className={cn('aui-md-sup', className)} {...props} />,
  pre: ({ className, ...props }) => <pre className={cn('aui-md-pre', className)} {...props} />,
  code: function Code({ className, ...props }) {
    const isCodeBlock = useIsMarkdownCodeBlock()
    return (
      <code className={cn(!isCodeBlock && 'aui-md-inline-code', className)} {...props} />
    )
  },
  CodeHeader,
})

export type MakeMarkdownTextProps = MarkdownTextPrimitiveProps

export function makeMarkdownText({
  className,
  components: userComponents,
  ...rest
}: MakeMarkdownTextProps = {}) {
  const components = {
    ...defaultMarkdownComponents,
    ...Object.fromEntries(
      Object.entries(userComponents ?? {}).filter(([, v]) => v !== undefined),
    ),
  }
  const MarkdownTextImpl = () => {
    const status = useSmoothStatus()
    return (
      <MarkdownTextPrimitive
        components={components}
        {...rest}
        className={cn(status.type === 'running' && 'aui-md-running', className)}
      />
    )
  }
  MarkdownTextImpl.displayName = 'MarkdownText'
  return memo(withSmoothContextProvider(MarkdownTextImpl), () => true)
}

// --- attachments ---

function useFileSrc(file: File | undefined) {
  const [src, setSrc] = useState<string | undefined>()
  useEffect(() => {
    if (!file) {
      setSrc(undefined)
      return
    }
    const objectUrl = URL.createObjectURL(file)
    setSrc(objectUrl)
    return () => URL.revokeObjectURL(objectUrl)
  }, [file])
  return src
}

function useAttachmentSrc() {
  const file = useAttachment((a) => (a.type === 'image' ? a.file : undefined))
  const remoteSrc = useAttachment((a) => {
    if (a.type !== 'image') return undefined
    const part = a.content?.find((c) => c.type === 'image')
    return part && part.type === 'image' ? part.image : undefined
  })
  const fileSrc = useFileSrc(file)
  return fileSrc ?? remoteSrc
}

function AttachmentPreview({ src }: { src: string }) {
  const [loaded, setLoaded] = useState(false)
  return (
    <img
      src={src}
      alt="Preview"
      style={{
        width: 'auto',
        height: 'auto',
        maxWidth: '75dvh',
        maxHeight: '75dvh',
        display: loaded ? 'block' : 'none',
        overflow: 'clip',
      }}
      onLoad={() => setLoaded(true)}
    />
  )
}

const AttachmentRoot = withDefaults(AttachmentPrimitive.Root, { className: 'aui-attachment-root' })
const AttachmentContent = withDefaults('div', { className: 'aui-attachment-content' })

function AttachmentRemove(
  props: React.ComponentProps<typeof TooltipIconButton>,
  ref: React.Ref<HTMLButtonElement>,
) {
  const {
    strings: { composer: { removeAttachment: { tooltip = 'Remove file' } = {} } = {} } = {},
  } = useThreadConfig()
  return (
    <AttachmentPrimitive.Remove asChild>
      <TooltipIconButton
        tooltip={tooltip}
        className="aui-attachment-remove"
        side="top"
        {...props}
        ref={ref}
      >
        {props.children ?? <CircleXIcon />}
      </TooltipIconButton>
    </AttachmentPrimitive.Remove>
  )
}
const AttachmentRemoveButton = forwardRef(AttachmentRemove)
AttachmentRemoveButton.displayName = 'AttachmentRemove'

function AttachmentUI() {
  const canRemove = useAttachment((a) => a.source !== 'message')
  const type = useAttachment((a) => a.type)
  const typeLabel =
    type === 'image' ? 'Image' : type === 'document' ? 'Document' : type === 'file' ? 'File' : 'File'
  const src = useAttachmentSrc()

  const thumb = (
    <div className="aui-attachment-thumb flex size-10 items-center justify-center rounded border bg-muted text-sm">
      {src ? (
        <img src={src} alt="" className="size-full rounded object-cover" />
      ) : (
        <FileIcon className="size-4" />
      )}
    </div>
  )

  const body = (
    <Tooltip>
      <AttachmentRoot>
        <TooltipTrigger asChild>
          <div className="aui-attachment-preview-trigger">
            {src ? (
              <Dialog>
                <DialogTrigger asChild>
                  <button type="button" className="contents">
                    <AttachmentContent>
                      {thumb}
                      <div className="aui-attachment-text">
                        <p className="aui-attachment-name">
                          <AttachmentPrimitive.Name />
                        </p>
                        <p className="aui-attachment-type">{typeLabel}</p>
                      </div>
                    </AttachmentContent>
                  </button>
                </DialogTrigger>
                <DialogContent>
                  <DialogTitle className="aui-sr-only">Image Attachment Preview</DialogTitle>
                  <AttachmentPreview src={src} />
                </DialogContent>
              </Dialog>
            ) : (
              <AttachmentContent>
                {thumb}
                <div className="aui-attachment-text">
                  <p className="aui-attachment-name">
                    <AttachmentPrimitive.Name />
                  </p>
                  <p className="aui-attachment-type">{typeLabel}</p>
                </div>
              </AttachmentContent>
            )}
          </div>
        </TooltipTrigger>
        <TooltipContent side="top">
          <AttachmentPrimitive.Name />
        </TooltipContent>
        {canRemove ? <AttachmentRemoveButton /> : null}
      </AttachmentRoot>
    </Tooltip>
  )

  return body
}
const Attachment = Object.assign(AttachmentUI, { Root: AttachmentRoot, Remove: AttachmentRemoveButton })

// --- message part (plain text for user bubbles) ---

const MessagePartText = withSmoothContextProvider(function MessagePartText() {
  const status = useSmoothStatus()
  return (
    <MessagePartPrimitive.Text
      className={cn('aui-text', status.type === 'running' && 'aui-text-running')}
      component="p"
    />
  )
})

const MessagePart = { Text: MessagePartText }

// --- branch picker ---

function useAllowBranchPicker(ensureCapability = false) {
  const { branchPicker: { allowBranchPicker = true } = {} } = useThreadConfig()
  const branchPickerSupported = useThread((t) => t.capabilities.edit)
  return allowBranchPicker && (!ensureCapability || branchPickerSupported)
}

const BranchPickerRoot = withDefaults(BranchPickerPrimitive.Root, {
  className: 'aui-branch-picker-root',
})

function BranchPickerDefault() {
  if (!useAllowBranchPicker(true)) return null
  return (
    <BranchPickerRoot hideWhenSingleBranch>
      <BranchPickerPrevious />
      <BranchPickerState />
      <BranchPickerNext />
    </BranchPickerRoot>
  )
}

const BranchPickerPrevious = forwardRef<HTMLButtonElement, Partial<TooltipIconButtonProps>>(
  (props, ref) => {
    const {
      strings: { branchPicker: { previous: { tooltip = 'Previous' } = {} } = {} } = {},
    } = useThreadConfig()
    const allow = useAllowBranchPicker()
    return (
      <BranchPickerPrimitive.Previous disabled={!allow} asChild>
        <TooltipIconButton tooltip={tooltip} {...props} ref={ref}>
          {props.children ?? <ChevronLeftIcon />}
        </TooltipIconButton>
      </BranchPickerPrimitive.Previous>
    )
  },
)
BranchPickerPrevious.displayName = 'BranchPickerPrevious'

const BranchPickerState = forwardRef<HTMLSpanElement, React.HTMLAttributes<HTMLSpanElement>>(
  (props, ref) => (
    <span className="aui-branch-picker-state" {...props} ref={ref}>
      <BranchPickerPrimitive.Number /> / <BranchPickerPrimitive.Count />
    </span>
  ),
)
BranchPickerState.displayName = 'BranchPickerState'

const BranchPickerNext = forwardRef<HTMLButtonElement, Partial<TooltipIconButtonProps>>(
  (props, ref) => {
    const {
      strings: { branchPicker: { next: { tooltip = 'Next' } = {} } = {} } = {},
    } = useThreadConfig()
    const allow = useAllowBranchPicker()
    return (
      <BranchPickerPrimitive.Next disabled={!allow} asChild>
        <TooltipIconButton tooltip={tooltip} {...props} ref={ref}>
          {props.children ?? <ChevronRightIcon />}
        </TooltipIconButton>
      </BranchPickerPrimitive.Next>
    )
  },
)
BranchPickerNext.displayName = 'BranchPickerNext'

export const BranchPicker = Object.assign(BranchPickerDefault, {
  Root: BranchPickerRoot,
  Previous: BranchPickerPrevious,
  Next: BranchPickerNext,
})

// --- assistant message ---

const AssistantMessageRoot = withDefaults(MessagePrimitive.Root, {
  className: 'aui-assistant-message-root',
})
const AssistantMessageContentWrapper = withDefaults('div', {
  className: 'aui-assistant-message-content',
})

const AssistantMessageContent = forwardRef<
  HTMLDivElement,
  MessagePrimitive.Parts.Props & React.HTMLAttributes<HTMLDivElement>
>(({ components: componentsProp, ...rest }, ref) => {
  const { tools, assistantMessage: { components = {} } = {} } = useThreadConfig()
  const toolsComponents = useMemo(
    () => ({
      by_name: !tools
        ? undefined
        : Object.fromEntries(tools.map((t) => [t.unstable_tool.toolName, t.unstable_tool.render])),
      Fallback: components.ToolFallback,
    }),
    [tools, components.ToolFallback],
  )
  const Footer = components.Footer
  return (
    <AssistantMessageContentWrapper {...rest} ref={ref}>
      <MessagePrimitive.Content
        components={{
          ...componentsProp,
          Text: componentsProp?.Text ?? components.Text ?? MessagePart.Text,
          Empty: componentsProp?.Empty ?? components.Empty,
          tools: toolsComponents,
        }}
      />
      {Footer ? <Footer /> : null}
    </AssistantMessageContentWrapper>
  )
})
AssistantMessageContent.displayName = 'AssistantMessageContent'

function AssistantMessageDefault() {
  return (
    <AssistantMessageRoot>
      <AssistantMessageContent />
      <BranchPicker />
    </AssistantMessageRoot>
  )
}

export const AssistantMessage = Object.assign(AssistantMessageDefault, {
  Root: AssistantMessageRoot,
  Content: AssistantMessageContent,
})

// --- user message ---

const UserMessageRoot = withDefaults(MessagePrimitive.Root, { className: 'aui-user-message-root' })
const UserMessageContentWrapper = withDefaults('div', { className: 'aui-user-message-content' })

const UserMessageContent = forwardRef<
  HTMLDivElement,
  MessagePrimitive.Parts.Props & React.HTMLAttributes<HTMLDivElement>
>(({ components, ...props }, ref) => (
  <UserMessageContentWrapper {...props} ref={ref}>
    <MessagePrimitive.Content
      components={{
        ...components,
        Text: components?.Text ?? MessagePart.Text,
      }}
    />
  </UserMessageContentWrapper>
))
UserMessageContent.displayName = 'UserMessageContent'

const UserMessageAttachmentsContainer = withDefaults('div', {
  className: 'aui-user-message-attachments',
})

function UserMessageAttachments({ components }: Partial<MessagePrimitive.Attachments.Props>) {
  return (
    <MessagePrimitive.If hasAttachments>
      <UserMessageAttachmentsContainer>
        <MessagePrimitive.Attachments
          components={{
            ...components,
            Attachment: components?.Attachment ?? Attachment,
          }}
        />
      </UserMessageAttachmentsContainer>
    </MessagePrimitive.If>
  )
}

function UserMessageDefault() {
  return (
    <UserMessageRoot>
      <UserMessageAttachments />
      <MessagePrimitive.If hasContent>
        <UserMessageContent />
      </MessagePrimitive.If>
      <BranchPicker />
    </UserMessageRoot>
  )
}

export const UserMessage = Object.assign(UserMessageDefault, {
  Root: UserMessageRoot,
  Content: UserMessageContent,
  Attachments: UserMessageAttachments,
})

// --- edit composer ---

const EditComposerRoot = withDefaults(ComposerPrimitive.Root, { className: 'aui-edit-composer-root' })
const EditComposerInput = withDefaults(ComposerPrimitive.Input, { className: 'aui-edit-composer-input' })
const EditComposerFooter = withDefaults('div', { className: 'aui-edit-composer-footer' })

function EditComposerDefault() {
  return (
    <EditComposerRoot>
      <EditComposerInput />
      <EditComposerFooter>
        <EditComposerCancel />
        <EditComposerSend />
      </EditComposerFooter>
    </EditComposerRoot>
  )
}

const EditComposerCancel = forwardRef<HTMLButtonElement, React.ComponentProps<typeof Button>>(
  (props, ref) => {
    const {
      strings: { editComposer: { cancel: { label = 'Cancel' } = {} } = {} } = {},
    } = useThreadConfig()
    return (
      <ComposerPrimitive.Cancel asChild>
        <Button variant="ghost" {...props} ref={ref}>
          {props.children ?? label}
        </Button>
      </ComposerPrimitive.Cancel>
    )
  },
)
EditComposerCancel.displayName = 'EditComposerCancel'

const EditComposerSend = forwardRef<HTMLButtonElement, React.ComponentProps<typeof Button>>(
  (props, ref) => {
    const {
      strings: { editComposer: { send: { label = 'Send' } = {} } = {} } = {},
    } = useThreadConfig()
    return (
      <ComposerPrimitive.Send asChild>
        <Button {...props} ref={ref}>
          {props.children ?? label}
        </Button>
      </ComposerPrimitive.Send>
    )
  },
)
EditComposerSend.displayName = 'EditComposerSend'

const EditComposer = Object.assign(EditComposerDefault, {
  Root: EditComposerRoot,
  Input: EditComposerInput,
  Footer: EditComposerFooter,
  Cancel: EditComposerCancel,
  Send: EditComposerSend,
})

// --- composer ---

function useAllowAttachments(ensureCapability = false) {
  const { composer: { allowAttachments = true } = {} } = useThreadConfig()
  const attachmentsSupported = useThread((t) => t.capabilities.attachments)
  return allowAttachments && (!ensureCapability || attachmentsSupported)
}

const ComposerRoot = withDefaults(ComposerPrimitive.Root, { className: 'aui-composer-root' })
const ComposerInputStyled = withDefaults(ComposerPrimitive.Input, {
  rows: 1,
  autoFocus: true,
  className: 'aui-composer-input',
})

const ComposerInput = forwardRef<
  HTMLTextAreaElement,
  React.ComponentProps<typeof ComposerPrimitive.Input>
>((props, ref) => {
  const {
    strings: { composer: { input: { placeholder = 'Write a message...' } = {} } = {} } = {},
  } = useThreadConfig()
  return <ComposerInputStyled placeholder={placeholder} {...props} ref={ref} />
})
ComposerInput.displayName = 'ComposerInput'

const ComposerAttachmentsContainer = withDefaults('div', { className: 'aui-composer-attachments' })

function ComposerAttachments({ components }: Partial<ComposerPrimitive.Attachments.Props>) {
  return (
    <ComposerAttachmentsContainer>
      <ComposerPrimitive.Attachments
        components={{
          ...components,
          Attachment: components?.Attachment ?? Attachment,
        }}
      />
    </ComposerAttachmentsContainer>
  )
}

const ComposerAttachButton = withDefaults(TooltipIconButton, {
  variant: 'default',
  className: 'aui-composer-attach',
})

const ComposerAddAttachment = forwardRef<HTMLButtonElement, Partial<TooltipIconButtonProps>>(
  (props, ref) => {
    const {
      strings: { composer: { addAttachment: { tooltip = 'Attach file' } = {} } = {} } = {},
    } = useThreadConfig()
    const allowAttachments = useAllowAttachments()
    return (
      <ComposerPrimitive.AddAttachment disabled={!allowAttachments} asChild>
        <ComposerAttachButton tooltip={tooltip} variant="ghost" {...props} ref={ref}>
          {props.children ?? <PaperclipIcon />}
        </ComposerAttachButton>
      </ComposerPrimitive.AddAttachment>
    )
  },
)
ComposerAddAttachment.displayName = 'ComposerAddAttachment'

function useAllowCancel() {
  return useThread((t) => t.capabilities.cancel)
}

const ComposerSendButton = withDefaults(TooltipIconButton, {
  variant: 'default',
  className: 'aui-composer-send',
})

const ComposerSend = forwardRef<HTMLButtonElement, Partial<TooltipIconButtonProps>>((props, ref) => {
  const {
    strings: { composer: { send: { tooltip = 'Send' } = {} } = {} } = {},
  } = useThreadConfig()
  return (
    <ComposerPrimitive.Send asChild>
      <ComposerSendButton tooltip={tooltip} {...props} ref={ref}>
        {props.children ?? <SendHorizontalIcon />}
      </ComposerSendButton>
    </ComposerPrimitive.Send>
  )
})
ComposerSend.displayName = 'ComposerSend'

const ComposerCancelButton = withDefaults(TooltipIconButton, {
  variant: 'default',
  className: 'aui-composer-cancel',
})

const ComposerCancel = forwardRef<HTMLButtonElement, Partial<TooltipIconButtonProps>>(
  (props, ref) => {
    const {
      strings: { composer: { cancel: { tooltip = 'Cancel' } = {} } = {} } = {},
    } = useThreadConfig()
    return (
      <ComposerPrimitive.Cancel asChild>
        <ComposerCancelButton tooltip={tooltip} {...props} ref={ref}>
          {props.children ?? <SquareIcon className="fill-current" />}
        </ComposerCancelButton>
      </ComposerPrimitive.Cancel>
    )
  },
)
ComposerCancel.displayName = 'ComposerCancel'

function ComposerAction() {
  const allowCancel = useAllowCancel()
  if (!allowCancel) return <ComposerSend />
  return (
    <>
      <ThreadPrimitive.If running={false}>
        <ComposerSend />
      </ThreadPrimitive.If>
      <ThreadPrimitive.If running>
        <ComposerCancel />
      </ThreadPrimitive.If>
    </>
  )
}

function ComposerDefault() {
  const allowAttachments = useAllowAttachments(true)
  return (
    <ComposerRoot>
      {allowAttachments ? <ComposerAttachments /> : null}
      {allowAttachments ? <ComposerAddAttachment /> : null}
      <ComposerInput autoFocus />
      <ComposerAction />
    </ComposerRoot>
  )
}

export const Composer = Object.assign(ComposerDefault, {
  Root: ComposerRoot,
  Input: ComposerInput,
  Action: ComposerAction,
  Send: ComposerSend,
  Cancel: ComposerCancel,
  AddAttachment: ComposerAddAttachment,
  Attachments: ComposerAttachments,
})

// --- thread ---

const ThreadRootStyled = withDefaults(ThreadPrimitive.Root, {
  className: 'aui-root aui-thread-root',
})

const ThreadRoot = forwardRef<
  HTMLDivElement,
  React.ComponentProps<typeof ThreadPrimitive.Root> & { config?: ThreadConfig; children?: React.ReactNode }
>(({ config, ...props }, ref) => (
  <ThreadConfigProvider config={config}>
    <ThreadRootStyled {...props} ref={ref} />
  </ThreadConfigProvider>
))
ThreadRoot.displayName = 'ThreadRoot'

export const ThreadViewport = withDefaults(ThreadPrimitive.Viewport, {
  className: 'aui-thread-viewport',
})

const ThreadViewportFooter = withDefaults('div', { className: 'aui-thread-viewport-footer' })

function ThreadMessages({
  components,
  MessagesFooter,
  unstable_flexGrowDiv: flexGrowDiv = true,
  ...rest
}: {
  unstable_flexGrowDiv?: boolean
  components?: Partial<ThreadPrimitive.Messages.Props['components']>
  MessagesFooter?: ComponentType | undefined
}) {
  return (
    <>
      <ThreadPrimitive.Messages
        components={{
          ...components,
          UserMessage: components?.UserMessage ?? UserMessage,
          AssistantMessage: components?.AssistantMessage ?? AssistantMessage,
          EditComposer: components?.EditComposer ?? EditComposer,
        }}
        {...rest}
      />
      {MessagesFooter ? <MessagesFooter /> : null}
      {flexGrowDiv ? (
        <ThreadPrimitive.If empty={false}>
          <div style={{ flexGrow: 1 }} />
        </ThreadPrimitive.If>
      ) : null}
    </>
  )
}

function ThreadFollowupSuggestions() {
  const suggestions = useThread((t) => t.suggestions)
  return (
    <ThreadPrimitive.If empty={false} running={false}>
      <div className="aui-thread-followup-suggestions">
        {suggestions?.map((suggestion, idx) => (
          <ThreadPrimitive.Suggestion
            key={idx}
            className="aui-thread-followup-suggestion"
            prompt={suggestion.prompt}
            method="replace"
            autoSend
          >
            {suggestion.prompt}
          </ThreadPrimitive.Suggestion>
        ))}
      </div>
    </ThreadPrimitive.If>
  )
}

const ThreadScrollToBottomButton = withDefaults(TooltipIconButton, {
  variant: 'outline',
  className: 'aui-thread-scroll-to-bottom',
})

const ThreadScrollToBottom = forwardRef<HTMLButtonElement, Partial<TooltipIconButtonProps>>(
  (props, ref) => {
    const {
      strings: { thread: { scrollToBottom: { tooltip = 'Scroll to bottom' } = {} } = {} } = {},
    } = useThreadConfig()
    return (
      <ThreadPrimitive.ScrollToBottom asChild>
        <ThreadScrollToBottomButton tooltip={tooltip} {...props} ref={ref}>
          {props.children}
        </ThreadScrollToBottomButton>
      </ThreadPrimitive.ScrollToBottom>
    )
  },
)
ThreadScrollToBottom.displayName = 'ThreadScrollToBottom'

function ThreadDefault(config: ThreadConfig) {
  const {
    components: {
      Composer: ComposerComponent = Composer,
      ThreadWelcome: ThreadWelcomeComponent = () => null,
      MessagesFooter,
      ...messageComponents
    } = {},
  } = config
  return (
    <ThreadRoot config={config}>
      <ThreadViewport>
        <ThreadWelcomeComponent />
        <ThreadMessages MessagesFooter={MessagesFooter} components={messageComponents} />
        <ThreadFollowupSuggestions />
        <ThreadViewportFooter>
          <ThreadScrollToBottom />
          <ComposerComponent />
        </ThreadViewportFooter>
      </ThreadViewport>
    </ThreadRoot>
  )
}

export const Thread = Object.assign(ThreadDefault, {
  Root: ThreadRoot,
  Viewport: ThreadViewport,
  Messages: ThreadMessages,
  FollowupSuggestions: ThreadFollowupSuggestions,
  ScrollToBottom: ThreadScrollToBottom,
  ViewportFooter: ThreadViewportFooter,
})
