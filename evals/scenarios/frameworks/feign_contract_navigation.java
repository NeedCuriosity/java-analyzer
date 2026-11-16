//? module api lib:spring
//? module user api lib:spring
//? module order api lib:spring
//- /org/springframework/web/bind/annotation/RequestMapping.java lib:spring
package org.springframework.web.bind.annotation;
public @interface RequestMapping { String[] value() default {}; String[] path() default {}; RequestMethod[] method() default {}; }
//- /org/springframework/web/bind/annotation/RequestMethod.java lib:spring
package org.springframework.web.bind.annotation;
public enum RequestMethod { GET, POST, PUT, DELETE }
//- /org/springframework/web/bind/annotation/GetMapping.java lib:spring
package org.springframework.web.bind.annotation;
public @interface GetMapping { String[] value() default {}; String[] path() default {}; }
//- /org/springframework/web/bind/annotation/PostMapping.java lib:spring
package org.springframework.web.bind.annotation;
public @interface PostMapping { String[] value() default {}; String[] path() default {}; }
//- /org/springframework/web/bind/annotation/PathVariable.java lib:spring
package org.springframework.web.bind.annotation;
public @interface PathVariable { String value() default ""; }
//- /org/springframework/web/bind/annotation/RestController.java lib:spring
package org.springframework.web.bind.annotation;
public @interface RestController {}
//- /org/springframework/cloud/openfeign/FeignClient.java lib:spring
package org.springframework.cloud.openfeign;
public @interface FeignClient { String value() default ""; String name() default ""; String url() default ""; String path() default ""; }
//- /api/src/main/java/demo/api/UserApi.java module:api
package demo.api;

import org.springframework.web.bind.annotation.*;

@RequestMapping("/api/users")
public interface UserApi {
    @GetMapping("/{id}")
    String /*@apiGet*/get(@PathVariable("id") long id);

    @PostMapping
    String /*@apiCreate*/create(String body);
}
//- /user/src/main/resources/application.yml module:user
spring:
  application:
    name: user-service
//- /user/src/main/java/demo/user/UserController.java module:user
package demo.user;

import demo.api.UserApi;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class UserController implements UserApi {
    @Override
    public String /*@srvGet*/get(long id) { return ""; }

    @Override
    public String /*@srvCreate*/create(String body) { return body; }

    public String helper() { return ""; }
}
//- /order/src/main/java/demo/order/UserClient.java module:order
package demo.order;

import demo.api.UserApi;
import org.springframework.cloud.openfeign.FeignClient;

@FeignClient("user-service")
public interface UserClient extends UserApi {
}
//- /order/src/main/java/demo/order/UserClientFallback.java module:order
package demo.order;

class UserClientFallback implements UserClient {
    public String /*@fbGet*/get(long id) { return "fallback"; }
    public String create(String body) { return "fallback"; }
}
//- /order/src/main/java/demo/order/OrderService.java module:order
package demo.order;

class OrderService {
    private final UserClient users;

    OrderService(UserClient users) { this.users = users; }

    String owner(long id) {
        return users./*@callGet*/get(id);
    }
}
//? impl callGet -> srvGet fbGet
//? refs srvGet = apiGet callGet
//? refs srvCreate = apiCreate
//? hover srvGet ~ "GET /api/users/{id}"
//? symbol "/api/users" "GET /api/users/{id}" srvGet
//? symbol "/api/users" "POST /api/users" srvCreate
//? diag none
