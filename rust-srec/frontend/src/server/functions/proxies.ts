import { z } from 'zod';
import { createServerFn } from '@/server/createServerFn';
import { parseInput } from '../validate';
import { fetchBackend } from '../api';
import { backendPath, PathIdSchema, withQuery } from '../backend-path';
import {
  CreateProxySchema,
  EffectiveRouteQuerySchema,
  EffectiveRouteSchema,
  ProbeOutcomeSchema,
  ProxyDetailSchema,
  ProxySchema,
  SystemProxySchema,
  TestProxySchema,
  UpdateProxySchema,
} from '@/api/schemas/proxies';

/** Saved proxies, with how many routes name each. */
export const listProxies = createServerFn({ method: 'GET' }).handler(async () =>
  z.array(ProxySchema).parse(await fetchBackend('/proxies')),
);

/** A saved proxy and the routes naming it. */
export const getProxy = createServerFn({ method: 'GET' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) =>
    ProxyDetailSchema.parse(await fetchBackend(backendPath`/proxies/${id}`)),
  );

export const createProxy = createServerFn({ method: 'POST' })
  .validator((input: z.input<typeof CreateProxySchema>) =>
    parseInput(CreateProxySchema, input),
  )
  .handler(async ({ data }) =>
    ProxySchema.parse(
      await fetchBackend('/proxies', {
        method: 'POST',
        body: JSON.stringify(data),
      }),
    ),
  );

/** An omitted password keeps the saved one; `username: null` removes the login. */
export const updateProxy = createServerFn({ method: 'POST' })
  .validator((input: z.input<typeof UpdateProxySchema>) =>
    parseInput(UpdateProxySchema, input),
  )
  .handler(async ({ data: { id, ...body } }) =>
    ProxySchema.parse(
      await fetchBackend(backendPath`/proxies/${id}`, {
        method: 'PATCH',
        body: JSON.stringify(body),
      }),
    ),
  );

const DeleteProxySchema = z.object({
  id: PathIdSchema,
  expected_version: z.number().int(),
});
export const deleteProxy = createServerFn({ method: 'POST' })
  .validator((input: z.infer<typeof DeleteProxySchema>) =>
    parseInput(DeleteProxySchema, input),
  )
  .handler(async ({ data: { id, expected_version } }) => {
    await fetchBackend(
      withQuery(
        backendPath`/proxies/${id}`,
        new URLSearchParams({ expected_version: String(expected_version) }),
      ),
      { method: 'DELETE' },
    );
  });

/** The proxy the server's environment configures, read when it started. */
export const getSystemProxy = createServerFn({ method: 'GET' }).handler(
  async () => SystemProxySchema.parse(await fetchBackend('/proxies/system')),
);

/** The route a scope's requests take now, and the setting that decided it. */
export const getEffectiveRoute = createServerFn({ method: 'GET' })
  .validator((input: z.infer<typeof EffectiveRouteQuerySchema>) =>
    parseInput(EffectiveRouteQuerySchema, input),
  )
  .handler(async ({ data }) => {
    const params = new URLSearchParams({ scope_type: data.scope_type });
    if (data.scope_id) params.set('scope_id', data.scope_id);
    if (data.platform_id) params.set('platform_id', data.platform_id);
    return EffectiveRouteSchema.parse(
      await fetchBackend(withQuery('/proxies/effective', params)),
    );
  });

/** Requests a platform's home page or a URL once through a proxy. */
export const testProxy = createServerFn({ method: 'POST' })
  .validator((input: z.input<typeof TestProxySchema>) =>
    parseInput(TestProxySchema, input),
  )
  .handler(async ({ data }) =>
    ProbeOutcomeSchema.parse(
      await fetchBackend('/proxies/test', {
        method: 'POST',
        body: JSON.stringify(data),
      }),
    ),
  );
