import { createFileRoute } from '@tanstack/react-router';

// A pathless layout with no component renders its children through the
// router's default <Outlet />.
export const Route = createFileRoute('/_public')({});
