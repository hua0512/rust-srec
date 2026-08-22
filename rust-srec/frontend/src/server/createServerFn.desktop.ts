type CreateServerFnOptions = {
  method?: string;
};

type HandlerContext<TInput> = {
  data: TInput;
};

type ServerFn<TInput, TOutput> = (
  opts?: { data: TInput } | TInput,
) => Promise<TOutput>;

type ValidatorFn<TInput, TOutput = TInput> =
  | ((data: TInput) => TOutput)
  | { parse: (data: TInput) => TOutput };

export interface ServerFnBuilder<TInput = void, TOutput = unknown> {
  validator<TNextInput>(
    validator: ValidatorFn<TNextInput, any>,
  ): ServerFnBuilder<TNextInput, TOutput>;
  handler<TResult extends TOutput>(
    fn: (ctx: HandlerContext<TInput>) => Promise<TResult> | TResult,
  ): ServerFn<TInput, TResult>;
  handler<TResult extends TOutput>(
    fn: () => Promise<TResult> | TResult,
  ): ServerFn<TInput, TResult>;
}

export function createServerFn<TInput = void, TOutput = unknown>(
  _opts?: CreateServerFnOptions,
): ServerFnBuilder<TInput, TOutput> {
  let activeValidator: ((data: any) => any) | null = null;

  const builder: ServerFnBuilder<any, any> = {
    validator(fn) {
      activeValidator =
        typeof fn === 'function'
          ? fn
          : (data: any) => ('parse' in fn ? fn.parse(data) : data);
      return builder;
    },
    handler(fn: any) {
      return (async (payload?: any) => {
        const rawData =
          payload && typeof payload === 'object' && 'data' in payload
            ? payload.data
            : payload;
        const validated = activeValidator ? activeValidator(rawData) : rawData;

        if (typeof fn === 'function' && fn.length === 0) {
          return await fn();
        }
        return await fn({ data: validated });
      }) as ServerFn<any, any>;
    },
  };

  return builder;
}
