//! The repository access-control rule.
//!
//! Every part of Gita that hands out repository content or accepts a
//! change to it — the REST API, Git over HTTP, Git over SSH — has to
//! answer the same question: *may this caller do this to this
//! repository?* The answer used to be re-derived at each of those
//! places, and the copies had drifted apart: the git transports keyed
//! on `owner_id` only, the REST read path looked at `is_private` only,
//! and the REST write path looked at `owner_id` only. A repository in a
//! group namespace did not fit any of them, because the group dimension
//! was missing from the model entirely.
//!
//! This module is the one place the rule is written down. It is a pure
//! function: it takes the repository, the caller, and what the caller
//! wants to do, and answers yes or no. Loading the facts it needs
//! (principally the caller's role in the repository's group) is the
//! caller's job — see `gita_db::Db::can_access`.
//!
//! # The rule
//!
//! Personal repositories:
//!
//! | Action | Who |
//! |---|---|
//! | Read | anyone, if the repository is public; the owner, if private |
//! | Write | the owner |
//! | Push | the owner |
//! | Admin | the owner |
//!
//! Repositories in a group namespace:
//!
//! | Action | Who |
//! |---|---|
//! | Read | anyone, if the repository is public; any member but a guest, if private |
//! | Write | any member but a guest |
//! | Push | members with the `owner`, `maintainer` or `developer` role |
//! | Admin | the group owner |
//!
//! The roles are the ones the API accepts (`maintainer`, `developer`,
//! `member`, `guest`), plus the `owner` role that `create_group` gives
//! the group's creator. A role nobody defined is read as `guest`, the
//! least privileged one.
//!
//! # Why the transports report failures differently
//!
//! The rule is shared; the *report* of a refusal is not, because the
//! protocols differ:
//!
//! * The REST API answers 404 for both "no such repository" and "not
//!   yours". Telling an anonymous caller that a repository exists but is
//!   hidden is itself a disclosure.
//! * Git over HTTP answers 401 with a `WWW-Authenticate` challenge for a
//!   private repository, because that is what makes a git client ask for
//!   credentials. Answering 404 would leave an authenticated user
//!   unable to clone a private repository they *are* allowed to clone:
//!   the client would never be prompted to send the token.
//! * Git over SSH has no status codes; it answers "permission denied"
//!   over the channel.
//!
//! All three are refusals of the same decision.

use crate::Repository;

/// What a caller wants to do with a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Browse the repository: read files, trees, commits, issues, pull
    /// requests, and clone it.
    Read,
    /// Create content that belongs to the repository: issues, comments,
    /// pull requests, stars, forks.
    Write,
    /// Change the repository's history or refs: push, create or delete
    /// branches and tags.
    Push,
    /// Change the repository's settings: rename, transfer, visibility,
    /// deletion, webhooks.
    Admin,
}

/// A caller's role in a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupRole {
    /// The group's creator. Full control over the group and its
    /// repositories, including their settings.
    Owner,
    /// May push code and administer the group's members, but not change
    /// the repositories' settings.
    Maintainer,
    /// May push code to the group's repositories.
    Developer,
    /// May read the group's private repositories and contribute content,
    /// but not push code.
    Member,
    /// May see the group's public repositories, and nothing else.
    Guest,
}

impl GroupRole {
    /// Parse a role as stored in `group_members.role`.
    ///
    /// Unknown roles are treated as [`GroupRole::Guest`]: the least
    /// privileged reading of a role nobody has defined. A typo in the
    /// database must not hand out more access than intended.
    pub fn parse(role: &str) -> Self {
        match role {
            "owner" => GroupRole::Owner,
            "maintainer" => GroupRole::Maintainer,
            "developer" => GroupRole::Developer,
            "member" => GroupRole::Member,
            _ => GroupRole::Guest,
        }
    }

    /// The role as stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            GroupRole::Owner => "owner",
            GroupRole::Maintainer => "maintainer",
            GroupRole::Developer => "developer",
            GroupRole::Member => "member",
            GroupRole::Guest => "guest",
        }
    }
}

/// Who is asking, and what they may do in the repository's group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Principal {
    /// The authenticated caller's user id, or `None` for an anonymous
    /// request.
    pub user_id: Option<i64>,
    /// The caller's role in the repository's group. `None` when the
    /// repository is personal, or when the caller is not a member of the
    /// group that owns it.
    pub group_role: Option<GroupRole>,
}

impl Principal {
    /// An anonymous caller.
    pub fn anonymous() -> Self {
        Self::default()
    }

    /// An authenticated caller with no group membership.
    pub fn user(user_id: i64) -> Self {
        Self {
            user_id: Some(user_id),
            group_role: None,
        }
    }

    /// The caller's user id, if they are authenticated.
    pub fn user_id(&self) -> Option<i64> {
        self.user_id
    }
}

/// Whether `principal` may perform `action` on `repo`.
///
/// This is the whole rule. See the module documentation for what it
/// means for each action.
pub fn can_access(repo: &Repository, principal: &Principal, action: Access) -> bool {
    match repo.group_id {
        // Personal repository: the owner may do anything; everyone else
        // may read it if it is public.
        None => {
            if principal.user_id == Some(repo.owner_id) {
                return true;
            }
            matches!(action, Access::Read) && !repo.is_private
        }
        // Repository in a group namespace: the group decides.
        Some(_) => match principal.group_role {
            // Not a member: a public repository is still readable, a
            // private one is not there for them.
            None => matches!(action, Access::Read) && !repo.is_private,
            Some(role) => match action {
                // A private repository is visible to the people who are
                // in it — but a guest is only there to look at the public
                // ones.
                Access::Read => !repo.is_private || role != GroupRole::Guest,
                // Any member but a guest may contribute content.
                Access::Write => role != GroupRole::Guest,
                // Pushing code needs a role that says so.
                Access::Push => matches!(
                    role,
                    GroupRole::Owner | GroupRole::Maintainer | GroupRole::Developer
                ),
                // Changing the repository's settings is the group
                // owner's business.
                Access::Admin => matches!(role, GroupRole::Owner),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn personal(is_private: bool) -> Repository {
        Repository {
            id: 1,
            owner_id: 10,
            owner_name: "alice".to_string(),
            name: "repo".to_string(),
            description: String::new(),
            is_private,
            default_branch: "main".to_string(),
            created_at: Utc::now(),
            group_id: None,
        }
    }

    fn group_repo(is_private: bool) -> Repository {
        Repository {
            group_id: Some(7),
            ..personal(is_private)
        }
    }

    fn principal(user_id: i64, role: Option<GroupRole>) -> Principal {
        Principal {
            user_id: Some(user_id),
            group_role: role,
        }
    }

    #[test]
    fn a_public_personal_repo_is_anonymously_readable() {
        let repo = personal(false);
        assert!(can_access(&repo, &Principal::anonymous(), Access::Read));
    }

    #[test]
    fn a_private_personal_repo_is_not_anonymously_readable() {
        let repo = personal(true);
        assert!(!can_access(&repo, &Principal::anonymous(), Access::Read));
        assert!(!can_access(&repo, &principal(11, None), Access::Read));
    }

    #[test]
    fn the_owner_may_do_everything_to_a_personal_repo() {
        for private in [false, true] {
            let repo = personal(private);
            let owner = principal(10, None);
            for action in [Access::Read, Access::Write, Access::Push, Access::Admin] {
                assert!(
                    can_access(&repo, &owner, action),
                    "owner denied {action:?} on a personal repo (private={private})"
                );
            }
        }
    }

    #[test]
    fn a_public_personal_repo_is_not_writable_by_others() {
        // Seeing a public repository does not make it yours to change.
        let repo = personal(false);
        let other = principal(11, None);
        assert!(can_access(&repo, &other, Access::Read));
        assert!(!can_access(&repo, &other, Access::Write));
        assert!(!can_access(&repo, &other, Access::Push));
        assert!(!can_access(&repo, &other, Access::Admin));
    }

    #[test]
    fn a_group_member_may_read_a_private_group_repo() {
        let repo = group_repo(true);
        let member = principal(11, Some(GroupRole::Member));
        assert!(can_access(&repo, &member, Access::Read));
    }

    #[test]
    fn a_non_member_may_not_read_a_private_group_repo() {
        let repo = group_repo(true);
        assert!(!can_access(&repo, &principal(11, None), Access::Read));
        assert!(!can_access(&repo, &Principal::anonymous(), Access::Read));
    }

    #[test]
    fn any_member_may_contribute_content_to_a_group_repo() {
        let repo = group_repo(true);
        let member = principal(11, Some(GroupRole::Member));
        assert!(can_access(&repo, &member, Access::Write));
    }

    #[test]
    fn only_developers_and_owners_may_push_to_a_group_repo() {
        let repo = group_repo(true);
        assert!(can_access(
            &repo,
            &principal(11, Some(GroupRole::Developer)),
            Access::Push
        ));
        assert!(can_access(
            &repo,
            &principal(12, Some(GroupRole::Owner)),
            Access::Push
        ));
        assert!(can_access(
            &repo,
            &principal(13, Some(GroupRole::Maintainer)),
            Access::Push
        ));
        assert!(!can_access(
            &repo,
            &principal(13, Some(GroupRole::Member)),
            Access::Push
        ));
        assert!(!can_access(
            &repo,
            &principal(13, Some(GroupRole::Guest)),
            Access::Push
        ));
    }

    #[test]
    fn a_guest_may_not_read_a_private_group_repo() {
        let repo = group_repo(true);
        let guest = principal(11, Some(GroupRole::Guest));
        assert!(!can_access(&repo, &guest, Access::Read));
        // ... but the public ones are fair game.
        let public = group_repo(false);
        assert!(can_access(&public, &guest, Access::Read));
    }

    #[test]
    fn a_guest_may_not_write_to_a_group_repo() {
        let repo = group_repo(false);
        let guest = principal(11, Some(GroupRole::Guest));
        assert!(!can_access(&repo, &guest, Access::Write));
        assert!(!can_access(&repo, &guest, Access::Admin));
    }

    #[test]
    fn a_public_group_repo_is_anonymously_readable() {
        let repo = group_repo(false);
        assert!(can_access(&repo, &Principal::anonymous(), Access::Read));
    }

    #[test]
    fn a_maintainer_may_push_but_not_administer() {
        let repo = group_repo(true);
        let maintainer = principal(11, Some(GroupRole::Maintainer));
        assert!(can_access(&repo, &maintainer, Access::Read));
        assert!(can_access(&repo, &maintainer, Access::Write));
        assert!(can_access(&repo, &maintainer, Access::Push));
        assert!(!can_access(&repo, &maintainer, Access::Admin));
    }

    #[test]
    fn only_the_group_owner_may_administer_a_group_repo() {
        let repo = group_repo(true);
        assert!(can_access(
            &repo,
            &principal(12, Some(GroupRole::Owner)),
            Access::Admin
        ));
        assert!(!can_access(
            &repo,
            &principal(11, Some(GroupRole::Developer)),
            Access::Admin
        ));
        assert!(!can_access(
            &repo,
            &principal(11, Some(GroupRole::Maintainer)),
            Access::Admin
        ));
    }

    #[test]
    fn an_unknown_role_is_treated_as_the_least_privileged_one() {
        assert_eq!(GroupRole::parse("owner"), GroupRole::Owner);
        assert_eq!(GroupRole::parse("maintainer"), GroupRole::Maintainer);
        assert_eq!(GroupRole::parse("developer"), GroupRole::Developer);
        assert_eq!(GroupRole::parse("member"), GroupRole::Member);
        assert_eq!(GroupRole::parse("guest"), GroupRole::Guest);
        assert_eq!(GroupRole::parse("superuser"), GroupRole::Guest);
        assert_eq!(GroupRole::parse(""), GroupRole::Guest);
    }
}
