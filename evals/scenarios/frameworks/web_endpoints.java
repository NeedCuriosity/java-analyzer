//- /org/springframework/web/bind/annotation/RequestMapping.java
package org.springframework.web.bind.annotation;
public @interface RequestMapping { String[] value() default {}; String[] path() default {}; RequestMethod[] method() default {}; }
//- /org/springframework/web/bind/annotation/RequestMethod.java
package org.springframework.web.bind.annotation;
public enum RequestMethod { GET, POST, PUT, DELETE }
//- /org/springframework/web/bind/annotation/GetMapping.java
package org.springframework.web.bind.annotation;
public @interface GetMapping { String[] value() default {}; }
//- /org/springframework/web/bind/annotation/PostMapping.java
package org.springframework.web.bind.annotation;
public @interface PostMapping { String[] value() default {}; }
//- /org/springframework/web/bind/annotation/DeleteMapping.java
package org.springframework.web.bind.annotation;
public @interface DeleteMapping { String[] value() default {}; String[] path() default {}; }
//- /org/springframework/web/bind/annotation/RestController.java
package org.springframework.web.bind.annotation;
public @interface RestController {}
//- /jakarta/ws/rs/Path.java
package jakarta.ws.rs;
public @interface Path { String value(); }
//- /jakarta/ws/rs/GET.java
package jakarta.ws.rs;
public @interface GET {}
//- /app/UserController.java
package app;

import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/api/users")
public class UserController {
    @GetMapping
    public String /*@list*/list() { return ""; }

    @GetMapping("/{id}")
    public String /*@get*/get(long id) { return ""; }

    @PostMapping(value = "/")
    public void /*@create*/create(String body) {}

    @DeleteMapping(path = {"/{id}", "/remove/{id}"})
    public void /*@remove*/remove(long id) {}

    @RequestMapping(value = "/search", method = {RequestMethod.GET, RequestMethod.POST})
    public String /*@search*/search(String q) { return q; }

    @RequestMapping("/any")
    public String /*@any*/any() { return ""; }

    public String notAnEndpoint() { return ""; }
}
//- /app/OrderResource.java
package app;

import jakarta.ws.rs.GET;
import jakarta.ws.rs.Path;

@Path("orders")
public class OrderResource {
    @GET
    @Path("{id}/items")
    public String /*@items*/items(long id) { return ""; }

    @GET
    public String /*@orders*/all() { return ""; }
}
//? symbol "/api/users" "GET /api/users" list
//? symbol "/api/users" "GET /api/users/{id}" get
//? symbol "/api/users" "POST /api/users" create
//? symbol "/remove" "DELETE /api/users/remove/{id}" remove
//? symbol "delete /api" "DELETE /api/users/{id}" remove
//? symbol "/search" "GET /api/users/search" search
//? symbol "/search" "POST /api/users/search" search
//? symbol "/any" "ANY /api/users/any" any
//? symbol "/orders" "GET /orders/{id}/items" items
//? symbol "/orders" "GET /orders" orders
//? diag none
