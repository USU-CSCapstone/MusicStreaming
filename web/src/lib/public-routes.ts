// The pages for someone who may not be signed in: the root load sends no one from them to the login
// page, adopts no device and loads no library, and the layout shows them on their own, without the
// app's shell. Only the login page still asks who is signed in, and sends them to `/`.

const PUBLIC_ROUTES = new Set(['/setup', '/login', '/invite']);

/** Whether the route with this ID is one of the public pages. */
export const isPublic = (routeId: string | null) => routeId !== null && PUBLIC_ROUTES.has(routeId);
