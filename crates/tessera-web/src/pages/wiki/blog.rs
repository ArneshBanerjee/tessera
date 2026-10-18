//! `/wiki/spaces/:key/blog`: a space's blog posts, newest first.

use leptos::prelude::*;
use leptos_router::components::A;

use super::{content_path, use_space};
use crate::api;
use crate::app::use_session;
use crate::components::Avatar;
use crate::model::{WikiCollection, WikiContent, short_time};

#[component]
pub fn BlogList() -> impl IntoView {
    let session = use_session();
    let space = use_space();
    let posts = LocalResource::new(move || {
        let key = space.key.get();
        session.changes.track();
        async move {
            let mut found = api::get::<WikiCollection<WikiContent>>(&format!(
                "{}/space/{key}/content/blogpost?expand=history,body.view&limit=200",
                api::WIKI
            ))
            .await?
            .results;
            // Dates are ISO 8601 in one time zone, so they sort as text.
            found.sort_by(|a, b| b.history.created_date.cmp(&a.history.created_date));
            Ok::<_, api::ApiError>(found)
        }
    });
    view! {
        <div class="page wiki-page">
            <header class="page-header">
                <h1>"Blog"</h1>
                <A href=move || format!("/wiki/spaces/{}/blog/create", space.key.get()) attr:class="button primary">
                    "Create blog post"
                </A>
            </header>
            <Transition fallback=|| view! { <p class="muted">"Loading blog posts…"</p> }>
                {move || posts.get().map(|result| match result {
                    Ok(list) if list.is_empty() => view! {
                        <p class="empty">"No blog posts yet. Share news and updates with the space."</p>
                    }.into_any(),
                    Ok(list) => view! {
                        <div class="blog-list">
                            {list.into_iter().map(|post| {
                                let href = content_path(&space.key.get_untracked(), "blogpost", &post.id);
                                let author = post.history.created_by.clone();
                                let html = post.view_html();
                                view! {
                                    <article class="blog-card">
                                        <h2><A href=href.clone()>{post.title.clone()}</A></h2>
                                        <p class="byline muted small">
                                            <Avatar user=Some(author.as_user()) size=20/>
                                            <span>{format!("{} · {}", author.display_name, short_time(&post.history.created_date))}</span>
                                        </p>
                                        // The server sanitizes the view format, so it is safe to insert as HTML.
                                        <div class="wiki-body blog-excerpt" inner_html=html></div>
                                        <A href=href attr:class="small">"Read more"</A>
                                    </article>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any(),
                    Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
                })}
            </Transition>
        </div>
    }
}
