//- /lombok/AccessLevel.java
package lombok;
public enum AccessLevel { PUBLIC, MODULE, PROTECTED, PACKAGE, PRIVATE, NONE }
//- /lombok/Getter.java
package lombok;
public @interface Getter { AccessLevel value() default AccessLevel.PUBLIC; boolean lazy() default false; }
//- /lombok/Setter.java
package lombok;
public @interface Setter { AccessLevel value() default AccessLevel.PUBLIC; }
//- /lombok/Data.java
package lombok;
public @interface Data { String staticConstructor() default ""; }
//- /lombok/NonNull.java
package lombok;
public @interface NonNull {}
//- /lombok/RequiredArgsConstructor.java
package lombok;
public @interface RequiredArgsConstructor { String staticName() default ""; AccessLevel access() default AccessLevel.PUBLIC; }
//- /lombok/AllArgsConstructor.java
package lombok;
public @interface AllArgsConstructor { String staticName() default ""; AccessLevel access() default AccessLevel.PUBLIC; }
//- /lombok/NoArgsConstructor.java
package lombok;
public @interface NoArgsConstructor { String staticName() default ""; AccessLevel access() default AccessLevel.PUBLIC; boolean force() default false; }
//- /lombok/experimental/Accessors.java
package lombok.experimental;
public @interface Accessors { boolean chain() default false; boolean fluent() default false; String[] prefix() default {}; }
//- /lombok/experimental/FieldDefaults.java
package lombok.experimental;
public @interface FieldDefaults { lombok.AccessLevel level() default lombok.AccessLevel.NONE; boolean makeFinal() default false; }
//- /lombok/experimental/NonFinal.java
package lombok.experimental;
public @interface NonFinal {}
//- /app/Account.java
package app;

import lombok.AccessLevel;
import lombok.Getter;
import lombok.Setter;

@Getter
@Setter
public class Account {
    @Getter(AccessLevel.NONE)
    private String /*@secret*/secret;
    @Setter(AccessLevel.PRIVATE)
    private long /*@balance*/balance;
    @Getter(value = AccessLevel.PROTECTED)
    private String owner;
}
//- /app/Point.java
package app;

import lombok.AccessLevel;
import lombok.AllArgsConstructor;
import lombok.NoArgsConstructor;
import lombok.RequiredArgsConstructor;

@RequiredArgsConstructor(staticName = "of")
@AllArgsConstructor(access = AccessLevel.PRIVATE)
@NoArgsConstructor(force = true)
public class /*@point_class*/Point {
    private final int x;
    private final int y;
    private final int scale = 1;
    private String label;
}
//- /app/Bean.java
package app;

import lombok.Getter;
import lombok.Setter;
import lombok.experimental.Accessors;

@Getter
@Setter
@Accessors(prefix = {"m", "f"}, chain = true)
public class Bean {
    private String mName;
    private boolean fActive;
}
//- /app/Service.java
package app;

import lombok.NonNull;
import lombok.RequiredArgsConstructor;
import lombok.experimental.FieldDefaults;
import lombok.experimental.NonFinal;

@FieldDefaults(level = lombok.AccessLevel.PRIVATE, makeFinal = true)
@RequiredArgsConstructor
public class Service {
    String name;
    java.util.List<String> cache = new java.util.ArrayList<>();
    @NonFinal int calls;
    @NonFinal @NonNull String tag;

    String describe() {
        calls++;
        return name + tag + cache.size();
    }
}
//- /app/Main.java
package app;

class Main {
    String run(Account account) {
        account.setOwner("me");
        account.setSecret("s");
        long b = account.getBalance();
        account./*@set_balance*/setBalance(b + 1);
        Point p = Point./*@of*/of(1, 2);
        Point q = new Point();
        Bean bean = new Bean().setName("n").setActive(true);
        Service service = new Service("svc", "tag");
        account./*@account_members*/hashCode();
        bean./*@bean_members*/hashCode();
        return bean.getName() + bean.isActive() + account.getOwner() + p + q + service.describe();
    }
}
//? diag inaccessible-member set_balance
//? diag exact
//? def of -> point_class
//? complete account_members has getBalance setSecret getOwner setOwner
//? complete account_members lacks getSecret
//? complete bean_members has getName setName isActive setActive
//? complete bean_members lacks getMName setMName getFActive isFActive
