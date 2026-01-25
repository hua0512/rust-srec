type CreateServerFnOptions = {
  method?: string;
};

type HandlerContext<TInput> = {
  data: TInput;
};

type ServerFn<TInput, TOutput> = (data?: TInput) => Promise<TOutput>;

type InputValidator<TInput> = (data: TInput) => TInput;

type ServerFnBuilder<TInput, TOutput> = {
  inputValidator: <TNextInput>(
    validator: InputValidator<TNextInput>,
  ) => ServerFnBuilder<TNextInput, TOutput>;
  handler: (
    handler: ((ctx: HandlerContext<TInput>) => Promise<TOutput> | TOutput) | (() => Promise<TOutput> | TOutput),
  ) => ServerFn<TInput, TOutput>;
};

export function createServerFn<TInput = void, TOutput = unknown>(
  _opts: CreateServerFnOptions,
): ServerFnBuilder<TInput, TOutput> {
  let validator: ((data: unknown) => unknown) | null = null;

  const builder: ServerFnBuilder<any, any> = {
    inputValidator(next) {
      validator = next as unknown as (data: unknown) => unknown;
      return builder;
    },
    handler(fn) {
      return (async (data?: any) => {
        const input =
          data && typeof data === 'object' && 'data' in data ? (data as any).data : data;
        const validated = validator ? validator(input) : input;
        // Allow handlers with either `(ctx) => ...` or `() => ...`.
        if (typeof fn === 'function' && fn.length === 0) {
          return await (fn as () => Promise<any> | any)();
        }
        return await (fn as (ctx: HandlerContext<any>) => Promise<any> | any)({
          data: validated,
        });
      }) as ServerFn<any, any>;
    },
  };

  return builder as unknown as ServerFnBuilder<TInput, TOutput>;
}
