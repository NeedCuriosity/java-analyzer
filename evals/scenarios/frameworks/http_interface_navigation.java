//? module client lib:spring
//? module server lib:spring
//? module jaxrs lib:spring
//- /org/springframework/web/bind/annotation/RequestMapping.java lib:spring
package org.springframework.web.bind.annotation;
public @interface RequestMapping { String[] value() default {}; String[] path() default {}; }
//- /org/springframework/web/bind/annotation/GetMapping.java lib:spring
package org.springframework.web.bind.annotation;
public @interface GetMapping { String[] value() default {}; }
//- /org/springframework/web/bind/annotation/PostMapping.java lib:spring
package org.springframework.web.bind.annotation;
public @interface PostMapping { String[] value() default {}; }
//- /org/springframework/web/bind/annotation/DeleteMapping.java lib:spring
package org.springframework.web.bind.annotation;
public @interface DeleteMapping { String[] value() default {}; }
//- /org/springframework/web/service/annotation/HttpExchange.java lib:spring
package org.springframework.web.service.annotation;
public @interface HttpExchange { String value() default ""; String url() default ""; String method() default ""; }
//- /org/springframework/web/service/annotation/GetExchange.java lib:spring
package org.springframework.web.service.annotation;
public @interface GetExchange { String value() default ""; String url() default ""; }
//- /org/springframework/web/service/annotation/PostExchange.java lib:spring
package org.springframework.web.service.annotation;
public @interface PostExchange { String value() default ""; String url() default ""; }
//- /jakarta/ws/rs/Path.java lib:spring
package jakarta.ws.rs;
public @interface Path { String value(); }
//- /jakarta/ws/rs/GET.java lib:spring
package jakarta.ws.rs;
public @interface GET {}
//- /server/src/main/java/demo/server/BookController.java module:server
package demo.server;

import org.springframework.web.bind.annotation.*;

@RequestMapping("/books/")
class BookController {
    @GetMapping("{isbn}")
    String /*@srvGet*/get(String isbn) { return isbn; }

    @GetMapping("/latest")
    String /*@srvLatest*/latest() { return ""; }

    @PostMapping
    String /*@srvAdd*/add(String book) { return book; }

    @DeleteMapping("/{isbn}")
    void /*@srvDelete*/delete(String isbn) {}
}
//- /jaxrs/src/main/java/demo/jaxrs/AuthorResource.java module:jaxrs
package demo.jaxrs;

import jakarta.ws.rs.GET;
import jakarta.ws.rs.Path;

@Path("/authors")
class AuthorResource {
    @GET
    @Path("{id}")
    String /*@srvAuthor*/author(long id) { return ""; }
}
//- /client/src/main/java/demo/client/BookApi.java module:client
package demo.client;

import org.springframework.web.service.annotation.*;

@HttpExchange(url = "http://books.local/books")
public interface BookApi {
    @GetExchange("/{id}")
    String /*@cliGet*/byIsbn(String id);

    @GetExchange("/*@cliLatestPath*/latest")
    String /*@cliLatest*/latest();

    @PostExchange
    String /*@cliAdd*/add(String book);

    @HttpExchange(method = "DELETE", url = "/{isbn}")
    void /*@cliDelete*/remove(String isbn);
}
//- /client/src/main/java/demo/client/AuthorApi.java module:client
package demo.client;

import org.springframework.web.service.annotation.GetExchange;

interface AuthorApi {
    @GetExchange("/authors/{id}")
    String /*@cliAuthor*/author(long id);
}
//- /client/src/main/java/demo/client/Reader.java module:client
package demo.client;

class Reader {
    String read(BookApi api, AuthorApi authors) {
        return api./*@callLatest*/latest() + authors.author(1);
    }
}
//? def cliGet -> srvGet
//? def cliLatest -> srvLatest
//? impl cliLatestPath -> srvLatest
//? impl callLatest -> srvLatest
//? def cliAdd -> srvAdd
//? def cliDelete -> srvDelete
//? def cliAuthor -> srvAuthor
//? refs srvGet = cliGet
//? refs srvLatest = cliLatest
//? refs srvDelete = cliDelete
//? refs srvAuthor = cliAuthor
//? hover cliLatest ~ "BookController.latest()"
//? symbol "/books" "GET /books/{isbn}" srvGet
//? diag none
