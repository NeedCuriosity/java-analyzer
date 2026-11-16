//? module order lib:spring
//? module user lib:spring
//? module admin lib:spring
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
//- /user/src/main/resources/application.yml module:user
spring:
  application:
    name: user-service
//- /user/src/main/java/demo/user/UserController.java module:user
package demo.user;

import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/api/users")
public class UserController {
    @GetMapping("/{id}")
    public String /*@srvGet*/get(@PathVariable long id) { return ""; }

    @PostMapping
    public String /*@srvCreate*/create(String body) { return body; }

    @GetMapping("/search")
    public String /*@srvSearch*/search(String q) { return q; }

    @RequestMapping(value = "/{id}/roles", method = RequestMethod.GET)
    public String /*@srvRoles*/roles(@PathVariable long id) { return ""; }
}
//- /admin/src/main/resources/application.properties module:admin
spring.application.name=admin-service
server.servlet.context-path=/admin
//- /admin/src/main/java/demo/admin/AdminUserController.java module:admin
package demo.admin;

import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/api/users")
public class AdminUserController {
    @GetMapping("/{userId:\\d+}")
    public String /*@adminGet*/find(@PathVariable long userId) { return ""; }

    @GetMapping("/api/stats")
    public String /*@adminStats*/stats() { return ""; }
}
//- /order/src/main/resources/application.yml module:order
spring:
  application:
    name: order-service
user:
  service: user-service
//- /order/src/main/java/demo/order/UserClient.java module:order
package demo.order;

import org.springframework.cloud.openfeign.FeignClient;
import org.springframework.web.bind.annotation.*;

@FeignClient(name = "user-service", path = "/api/users")
public interface UserClient {
    @GetMapping("/*@cliGetPath*/{id}")
    String /*@cliGet*/get(@PathVariable("id") long id);

    @PostMapping
    String /*@cliCreate*/create(String body);

    @GetMapping("/search")
    String /*@cliSearch*/search(String q);

    @RequestMapping(path = "/{id}/roles", method = RequestMethod.GET)
    String /*@cliRoles*/roles(@PathVariable("id") long id);

    @GetMapping("/{id}/orders")
    String /*@cliMissing*/missing();
}
//- /order/src/main/java/demo/order/AdminClient.java module:order
package demo.order;

import org.springframework.cloud.openfeign.FeignClient;
import org.springframework.web.bind.annotation.GetMapping;

@FeignClient(value = "admin-service", url = "http://localhost:8082/admin")
interface AdminClient {
    @GetMapping("/api/users/api/stats")
    String /*@cliStats*/stats();
}
//- /order/src/main/java/demo/order/PlaceholderClient.java module:order
package demo.order;

import org.springframework.cloud.openfeign.FeignClient;
import org.springframework.web.bind.annotation.GetMapping;

@FeignClient("${user.service:other}")
interface PlaceholderClient {
    @GetMapping("/api/users/{id}")
    String /*@cliPlaceholder*/byId(long id);
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
//? def cliGet -> srvGet
//? def cliGetPath -> srvGet
//? impl cliGet -> srvGet
//? def callGet -> cliGet
//? impl callGet -> srvGet
//? def cliCreate -> srvCreate
//? def cliSearch -> srvSearch
//? def cliRoles -> srvRoles
//? def cliMissing -> cliMissing
//? def cliStats -> adminStats
//? def cliPlaceholder -> srvGet
//? refs srvGet = cliGet cliPlaceholder
//? refs srvSearch = cliSearch
//? refs adminGet =
//? refs adminStats = cliStats
//? hover cliGet ~ "UserController.get()"
//? hover srvGet ~ "GET /api/users/{id}"
//? symbol "/api/users" "GET /api/users/{id}" srvGet
//? diag none
