//- /org/springframework/stereotype/Component.java lib:spring
package org.springframework.stereotype;
public @interface Component { String value() default ""; }
//- /org/springframework/stereotype/Service.java lib:spring
package org.springframework.stereotype;
@Component
public @interface Service { String value() default ""; }
//- /org/springframework/stereotype/Controller.java lib:spring
package org.springframework.stereotype;
@Component
public @interface Controller { String value() default ""; }
//- /org/springframework/context/annotation/Configuration.java lib:spring
package org.springframework.context.annotation;
public @interface Configuration { String value() default ""; }
//- /org/springframework/context/annotation/Bean.java lib:spring
package org.springframework.context.annotation;
public @interface Bean { String[] value() default {}; String[] name() default {}; }
//- /org/springframework/context/annotation/Primary.java lib:spring
package org.springframework.context.annotation;
public @interface Primary {}
//- /org/springframework/beans/factory/annotation/Autowired.java lib:spring
package org.springframework.beans.factory.annotation;
public @interface Autowired { boolean required() default true; }
//- /org/springframework/beans/factory/annotation/Qualifier.java lib:spring
package org.springframework.beans.factory.annotation;
public @interface Qualifier { String value() default ""; }
//- /jakarta/annotation/Resource.java lib:spring
package jakarta.annotation;
public @interface Resource { String name() default ""; }
//- /org/springframework/data/repository/Repository.java lib:spring
package org.springframework.data.repository;
public interface Repository<T, ID> {}
//- /org/springframework/data/repository/CrudRepository.java lib:spring
package org.springframework.data.repository;
public interface CrudRepository<T, ID> extends Repository<T, ID> { T save(T entity); }
//- /lombok/RequiredArgsConstructor.java lib:lombok
package lombok;
public @interface RequiredArgsConstructor {}
//- /src/main/java/demo/DomainService.java
package demo;

import org.springframework.stereotype.Service;

@Service
public @interface DomainService {}
//- /src/main/java/demo/Notifier.java
package demo;

public interface Notifier {
    void send(String message);
}
//- /src/main/java/demo/EmailNotifier.java
package demo;

import org.springframework.context.annotation.Primary;
import org.springframework.stereotype.Component;

@Primary
@Component
public class /*@email*/EmailNotifier implements Notifier {
    public void send(String message) {}
}
//- /src/main/java/demo/SmsNotifier.java
package demo;

import org.springframework.stereotype.Component;

@Component("sms")
public class /*@sms*/SmsNotifier implements Notifier {
    public void send(String message) {}
}
//- /src/main/java/demo/PushNotifier.java
package demo;

@DomainService
public class /*@push*/PushNotifier implements Notifier {
    public void send(String message) {}
}
//- /src/main/java/demo/Formatter.java
package demo;

import org.springframework.stereotype.Component;

public interface Formatter {
    String format(Object value);

    @Component
    class /*@json*/JsonFormatter implements Formatter {
        public String format(Object value) { return "{}"; }
    }

    @Component
    class /*@xml*/XmlFormatter implements Formatter {
        public String format(Object value) { return "<x/>"; }
    }
}
//- /src/main/java/demo/Owner.java
package demo;

public class Owner {}
//- /src/main/java/demo/Store.java
package demo;

import org.springframework.stereotype.Component;

public interface Store<T> {
    @Component
    class /*@ownerStore*/OwnerStore implements Store<Owner> {}

    @Component
    class /*@stringStore*/StringStore implements Store<String> {}
}
//- /src/main/java/demo/OwnerRepository.java
package demo;

import org.springframework.data.repository.CrudRepository;

public interface /*@ownerRepo*/OwnerRepository extends CrudRepository<Owner, Integer> {}
//- /src/main/java/demo/AppConfig.java
package demo;

import java.time.Clock;

import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Configuration;

@Configuration
public class AppConfig {
    @Bean
    public Clock /*@clockBean*/clock() {
        return Clock.systemUTC();
    }

    @Bean(name = "auditNotifier")
    Notifier /*@auditBean*/audit(Clock /*@clockParam*/clock) {
        return message -> {};
    }
}
//- /src/main/java/demo/OwnerService.java
package demo;

import java.util.List;
import java.util.Map;

import jakarta.annotation.Resource;
import lombok.RequiredArgsConstructor;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.beans.factory.annotation.Qualifier;
import org.springframework.stereotype.Service;

@Service
@RequiredArgsConstructor
public class /*@ownerService*/OwnerService {
    private final /*@repoType*/OwnerRepository /*@repoField*/owners;
    private final Notifier /*@primaryField*/notifier;
    private final String label = "owners";

    @Autowired
    @Qualifier("sms")
    private Notifier /*@smsField*/sms;

    @Resource(name = "auditNotifier")
    private Notifier /*@resField*/audit;

    @Autowired
    private List<Notifier> /*@allField*/all;

    @Autowired
    private Map<String, Notifier> /*@mapField*/byName;

    @Autowired
    private Formatter /*@xmlByName*/xmlFormatter;

    @Autowired
    private Store<Owner> /*@storeField*/store;

    @Autowired
    private Store<?> /*@anyStoreField*/anyStore;

    private Formatter formatter;

    @Autowired
    void setFormatter(Formatter /*@setterParam*/formatter) {
        this.formatter = formatter;
    }

    void register(Owner owner) {
        owners.save(owner);
        /*@useNotifier*/notifier.send(label);
    }
}
//- /src/main/java/demo/OwnerController.java
package demo;

import java.time.Clock;

import org.springframework.stereotype.Controller;

@Controller
public class OwnerController {
    private final /*@serviceType*/OwnerService /*@serviceField*/service;
    private final Clock /*@clockField*/clock;

    public OwnerController(/*@serviceType2*/OwnerService /*@ctorParam*/service, Clock /*@ctorClock*/clock) {
        this.service = service;
        this.clock = clock;
    }
}
//- /src/main/java/demo/Plain.java
package demo;

class Plain {
    Plain(/*@serviceType3*/OwnerService /*@plainParam*/owners) {}
}
//- /src/test/java/demo/TestNotifier.java root:test
package demo;

import org.springframework.stereotype.Component;

@Component
class /*@testNotifier*/TestNotifier implements Notifier {
    public void send(String message) {}
}
//- /src/test/java/demo/OwnerServiceTest.java root:test
package demo;

import java.util.List;

import org.springframework.beans.factory.annotation.Autowired;

class OwnerServiceTest {
    @Autowired
    List<Notifier> /*@testAll*/notifiers;
}
//? impl repoField -> ownerRepo
//? impl primaryField -> email
//? impl useNotifier -> email
//? impl smsField -> sms
//? impl resField -> auditBean
//? impl allField -> email sms push auditBean
//? impl mapField -> email sms push auditBean
//? impl testAll -> email sms push auditBean testNotifier
//? impl xmlByName -> xml
//? impl storeField -> ownerStore
//? impl anyStoreField -> ownerStore stringStore
//? impl setterParam -> json xml
//? impl ctorParam -> ownerService
//? impl ctorClock -> clockBean
//? impl serviceField -> ownerService
//? impl clockField -> clockBean
//? impl clockParam -> clockBean
//? impl plainParam -> plainParam
//? refs email = primaryField allField mapField testAll
//? refs push = allField mapField testAll
//? refs clockBean = ctorClock clockParam
//? refs auditBean = resField allField mapField testAll
//? refs ownerRepo = repoType repoField
//? refs ownerService = serviceType serviceType2 ctorParam serviceType3
//? hover primaryField ~ "`emailNotifier` → `EmailNotifier`"
//? hover ctorClock ~ "`clock` → `AppConfig.clock()`"
//? hover resField ~ "`auditNotifier` → `AppConfig.audit()`"
//? hover sms ~ "Bean `sms`"
//? diag none
