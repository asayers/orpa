<h1 align="center">Orpa</h1>

**tl;dr**: Do you try to review your colleagues' commits, but sometimes they
slip past you?  Are you tired of re-reviewing commits you've already looked at?
Keep track of the ones you've reviewed and you'll see them exactly once!

This repo contains a tool called `orpa` which is designed to streamline a
certain workflow.  Read [Tracking read/unread commits](reviewing_commits.md)
and [Reviewing merge requests](reviewing_mrs.md) for a description of that
workflow.

## Listing unreviewed commits

`orpa` shows you the commits which don't yet have any notes attached:

```
$ orpa status
Current branch: The following commits are awaiting review:

  7cc8026 Use the gitlab raw Query API
  251ec84 Replace coloured with yansi
  da05da1 Document the CLI options
```

Use `orpa list` to get the revisions in a form suitable for
machine-consumption:

```
$ orpa list
7cc80264ffda6c7aa768e62a8640684be6361111
251ec841a88fa999a0b9dde4f17a22cd2d784602
da05da1878353964048388427fcbb18ef3314bf1
```

I like to pipe this into `tig` (you need to pass `--no-walk` to make sure
tig only lists the specified commits).  Try putting this in your gitconfig:

```
[alias]
    review = !sh -c 'orpa list | tig --no-walk --stdin'
```

Now `git review` shows a list of all the unreviewed commits, and you can
blast through them by hitting 'T'.

Both commands will accept a range, so you can use them with merge requests too:

```
$ orpa status 563e5fb..aadb1f9
563e5fb..aadb1f9: The following commits are awaiting review:

  9fbc3f8 Make the notes ref configurable
  aadb1f9 Use Lazy for CLI opts
```

## Listing merge requests

Orpa can load the open MRs from your MR tracker and display the unreviewed
commits in the same way.  Currently it only supports gitlab, but support
for other trackers could be added too.

Let's grab the latest MRs from gitlab with `orpa fetch`:

```
$ orpa fetch
Fetching open MRs for project 1 from gitlab.example.com...
```

Now, `orpa status` is giving us some new information:

```
$ orpa status
Merge requests with unreviewed commits:

  !84    Add --notes-ref CLI argument (2 unreviewed)

Use "orpa mr" to see the full MR information
```

Let's take a look at it...

```
$ orpa mr 84
merge_request !84
Author: Joe Smith (@jsmith)
Date:   2019-12-10 08:42:20.768 UTC

    Add --notes-ref CLI argument

    v1 563e5fb..aadb1f9 (0/2 reviewed)
```

And there's the range we need to review!

We review it, send some feedback, and then later run `orpa fetch` again.
Now, we see:

```
$ orpa mr 84
merge_request !84
Author: Joe Smith (@jsmith)
Date:   2019-12-10 08:42:20.768 UTC

    Add --notes-ref CLI argument

    v1 563e5fb..aadb1f9 (2/2 reviewed)
    v2 c3f89fb..bfb0da1 (0/3 reviewed)
```

Now we can see the old reviewed range, as well as the new unreviewed range -
everything we need to run `git range-diff`.

### Configuring `orpa fetch`

Get an API token for your gitlab instance (read-only API access is enough),
and put a section like this in your local git repository's .git/config file:

```ini
[gitlab]
    url = "gitlab.com"
    privateToken = "1234567890abcdefgijk"
    projectId = "8765"
    username = "asayers"
```
