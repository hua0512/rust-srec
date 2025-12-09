export function Footer() {
    return (
        <footer className="border-t py-6 md:py-0">
            <div className='z-20 w-full bg-background/95 shadow backdrop-blur supports-[backdrop-filter]:bg-background/60'>
                <div className='mx-4 flex h-14 items-center md:mx-8'>
                    <p className='text-left text-xs leading-loose text-muted-foreground md:text-sm'>
                        Powered by{" "}
                        <a
                            href='https://github.com/stream-rec/stream-rec'
                            target='_blank'
                            rel='noopener noreferrer'
                            className='font-medium underline underline-offset-4'
                        >
                            Stream-rec
                        </a>
                        . WebUI version: 0.3.6
                    </p>
                </div>
            </div>
        </footer>
    );
}
