//! Which libraries an account reaches (`requirements/users.md` §5): the one test every query
//! that keeps libraries apart (`requirements/general.md` §3.6) makes.

/// SQL that is true where the account whose ID is the SQL `user` reaches the library whose ID
/// is the SQL `library`: always if the SQL `admin` is true, as it is for an admin or the owner,
/// and otherwise only if they were granted it. `admin` is `u.role <> 'user'` where the account's
/// row is at hand, or a bound `Caller::admin`. A `library` that does not exist is reached by an
/// admin, so a caller that needs it to exist checks that too.
///
/// `user` and `library` must each be a bound parameter or a column qualified by its table,
/// never a bare column name: a bare `library_id` or `user_id` names `library_access`'s own
/// column, which makes the test true for every library anyone was granted.
macro_rules! reaches {
    (admin: $admin:literal, user: $user:literal, library: $library:literal) => {
        concat!(
            "(",
            $admin,
            " OR EXISTS (SELECT 1 FROM library_access WHERE user_id = ",
            $user,
            " AND library_id = ",
            $library,
            "))"
        )
    };
}
pub(crate) use reaches;
