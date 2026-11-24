// V2-A08 parity fixture: a §9.1 loader with one `defer()`ed value. Pins the
// deferred-envelope shape (`data` carries the `{ __deferred: id }` marker,
// `deferredPending` lists the id) on both hosts without awaiting the promise.
export const load = async (event) => {
  const slow = event.defer(Promise.resolve('later'));
  return { status: 200, data: { now: 'fast', slow } };
};
